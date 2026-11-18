//! Which local model fits this computer, and how to load it.
//!
//! Deuri runs a model through LM Studio, and everyone's machine is different.
//! This module looks at the hardware (GPU and its memory, system RAM, CPU),
//! reads each model's architecture from its GGUF header, and works out load
//! settings that fit: how many tokens of context, how much of the model goes
//! on the GPU, and whether to quantize the K/V cache.
//!
//! The memory model is deliberately simple and a little conservative:
//!   weights      = the GGUF file size
//!   K/V cache    = tokens x layers-with-attention x kv-heads x (key + value dims) x bytes per element
//!   GPU share    = the fraction of layers offloaded, applied to both
//! plus a fixed reserve for the display, the runtime and compute buffers.

use std::io::{BufReader, Read, Seek};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
/// VRAM kept free for the desktop, the CUDA/Vulkan runtime and compute buffers.
const VRAM_RESERVE: f64 = 1.3 * GIB;
/// RAM kept free for Windows, the browser and everything else.
const RAM_RESERVE: f64 = 6.0 * GIB;
/// Largest context the recommender asks for; beyond this, prompt processing
/// gets slow and few browsing tasks need it.
const MAX_WANTED_CTX: u64 = 131_072;
/// Smallest context that still leaves room for a page plus a conversation.
const COMFORT_CTX: u64 = 32_768;
const CTX_STEPS: &[u64] = &[131_072, 100_000, 65_536, 49_152, 32_768, 24_576, 16_384, 12_288, 8_192];

/// The hardware Deuri has to work with.
#[derive(Debug, Clone, Default)]
pub struct System {
    pub cpu: String,
    pub threads: usize,
    pub ram_bytes: u64,
    /// Dedicated GPU, if any (integrated graphics share system RAM and are
    /// treated as no GPU).
    pub gpu: Option<Gpu>,
}

#[derive(Debug, Clone, Default)]
pub struct Gpu {
    pub name: String,
    pub vram_bytes: u64,
    /// Free right now (NVIDIA only); other programs may be using some.
    pub free_bytes: Option<u64>,
}

impl System {
    pub fn to_json(&self) -> Value {
        json!({
            "cpu": self.cpu, "threads": self.threads, "ram_gb": gb(self.ram_bytes as f64),
            "gpu": self.gpu.as_ref().map(|g| json!({
                "name": g.name, "vram_gb": gb(g.vram_bytes as f64),
                "free_gb": g.free_bytes.map(|f| gb(f as f64)),
            })),
        })
    }
}

fn gb(bytes: f64) -> f64 {
    (bytes / GIB * 10.0).round() / 10.0
}

fn quiet(cmd: &mut Command) -> Option<String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Look at this computer's CPU, RAM and GPU.
pub fn detect() -> System {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    System { cpu: cpu_name(), threads, ram_bytes: ram_bytes(), gpu: nvidia().or_else(other_gpu) }
}

/// NVIDIA cards report exact total and free memory through nvidia-smi.
fn nvidia() -> Option<Gpu> {
    let out = quiet(Command::new("nvidia-smi").args(["--query-gpu=name,memory.total,memory.free", "--format=csv,noheader,nounits"]))?;
    out.lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split(',').map(str::trim).collect();
            let total: u64 = p.get(1)?.parse().ok()?;
            let free: Option<u64> = p.get(2).and_then(|f| f.parse().ok());
            Some(Gpu { name: p[0].to_string(), vram_bytes: total * 1024 * 1024, free_bytes: free.map(|f| f * 1024 * 1024) })
        })
        .max_by_key(|g| g.vram_bytes)
}

/// Integrated graphics: no memory of their own worth offloading to.
fn is_integrated(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    // Intel's discrete cards carry a model number (Arc A770, Arc B580, Arc Pro
    // B60); the Core Ultra integrated GPU is just "Arc Graphics".
    let intel_discrete = n.contains("arc")
        && n.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| w.len() >= 3 && (w.starts_with('a') || w.starts_with('b')) && w[1..].chars().all(|c| c.is_ascii_digit()));
    (n.contains("intel") && !intel_discrete)
        || n.contains("radeon(tm) graphics")
        || n.contains("radeon graphics")
        || n.contains("vega")
        || n.contains("virtual")
        || n.contains("basic display")
        || n.contains("basic render")
}

/// AMD / Intel Arc: dedicated memory from the display driver's registry key
/// (64-bit, unlike WMI's AdapterRAM which stops at 4 GB).
#[cfg(windows)]
fn other_gpu() -> Option<Gpu> {
    let out = quiet(Command::new("reg").args([
        "query",
        r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}",
        "/s",
    ]))?;
    let mut best: Option<Gpu> = None;
    let mut name = String::new();
    let mut mem = 0u64;
    let flush = |name: &mut String, mem: &mut u64, best: &mut Option<Gpu>| {
        if !name.is_empty() && *mem > 0 && !is_integrated(name) && best.as_ref().is_none_or(|b| *mem > b.vram_bytes) {
            *best = Some(Gpu { name: name.clone(), vram_bytes: *mem, free_bytes: None });
        }
        name.clear();
        *mem = 0;
    };
    for line in out.lines() {
        let t = line.trim();
        if t.starts_with("HKEY_") {
            flush(&mut name, &mut mem, &mut best);
            continue;
        }
        let parts: Vec<&str> = t.split_whitespace().collect();
        if parts.len() >= 3 && parts[0] == "DriverDesc" {
            name = parts[2..].join(" ");
        } else if parts.len() >= 3 && parts[0] == "HardwareInformation.qwMemorySize" {
            mem = u64::from_str_radix(parts[2].trim_start_matches("0x"), 16).unwrap_or(0);
        }
    }
    flush(&mut name, &mut mem, &mut best);
    best
}

#[cfg(not(windows))]
fn other_gpu() -> Option<Gpu> {
    None
}

#[cfg(windows)]
fn cpu_name() -> String {
    quiet(Command::new("reg").args(["query", r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0", "/v", "ProcessorNameString"]))
        .and_then(|o| o.lines().find(|l| l.contains("ProcessorNameString")).map(|l| l.split("REG_SZ").nth(1).unwrap_or("").trim().to_string()))
        .unwrap_or_default()
}

#[cfg(not(windows))]
fn cpu_name() -> String {
    String::new()
}

#[cfg(windows)]
fn ram_bytes() -> u64 {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: `m` is a valid, correctly sized MEMORYSTATUSEX.
    unsafe { GlobalMemoryStatusEx(&mut m) }.map(|_| m.ullTotalPhys).unwrap_or(0)
}

#[cfg(not(windows))]
fn ram_bytes() -> u64 {
    0
}

/// What a model's GGUF header says about its shape.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Arch {
    pub name: String,
    pub layers: u64,
    /// Layers that keep a K/V cache (hybrid models such as Qwen 3.5 use
    /// attention only every few layers).
    pub attn_layers: u64,
    pub kv_heads: u64,
    pub key_dim: u64,
    pub value_dim: u64,
    pub train_ctx: u64,
}

impl Arch {
    /// K/V cache elements stored per token of context.
    pub fn kv_elems_per_token(&self) -> u64 {
        self.attn_layers * self.kv_heads * (self.key_dim + self.value_dim)
    }
}

#[derive(Debug, Clone)]
enum Gv {
    Int(u64),
    Str(String),
    Ints(Vec<u64>),
    Other,
}

fn rd<const N: usize>(r: &mut impl Read) -> std::io::Result<[u8; N]> {
    let mut b = [0u8; N];
    r.read_exact(&mut b)?;
    Ok(b)
}

fn rd_u32(r: &mut impl Read) -> std::io::Result<u32> {
    Ok(u32::from_le_bytes(rd::<4>(r)?))
}

fn rd_u64(r: &mut impl Read) -> std::io::Result<u64> {
    Ok(u64::from_le_bytes(rd::<8>(r)?))
}

fn bad(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string())
}

fn scalar_size(t: u32) -> Option<i64> {
    Some(match t {
        0 | 1 | 7 => 1,
        2 | 3 => 2,
        4..=6 => 4,
        10..=12 => 8,
        _ => return None,
    })
}

fn rd_int(r: &mut impl Read, t: u32) -> std::io::Result<Option<u64>> {
    Ok(match t {
        0 => Some(rd::<1>(r)?[0] as u64),
        1 => Some((rd::<1>(r)?[0] as i8).max(0) as u64),
        2 => Some(u16::from_le_bytes(rd::<2>(r)?) as u64),
        3 => Some(i16::from_le_bytes(rd::<2>(r)?).max(0) as u64),
        4 => Some(rd_u32(r)? as u64),
        5 => Some(i32::from_le_bytes(rd::<4>(r)?).max(0) as u64),
        10 => Some(rd_u64(r)?),
        11 => Some(i64::from_le_bytes(rd::<8>(r)?).max(0) as u64),
        _ => None,
    })
}

fn rd_value<R: Read + Seek>(r: &mut R, t: u32) -> std::io::Result<Gv> {
    match t {
        8 => {
            let n = rd_u64(r)?;
            if n > 1 << 20 {
                return Err(bad("string too long"));
            }
            let mut b = vec![0u8; n as usize];
            r.read_exact(&mut b)?;
            Ok(Gv::Str(String::from_utf8_lossy(&b).into_owned()))
        }
        9 => {
            let et = rd_u32(r)?;
            let n = rd_u64(r)?;
            if let Some(sz) = scalar_size(et) {
                if n <= 1024 && matches!(et, 0..=5 | 10 | 11) {
                    let mut v = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        v.push(rd_int(r, et)?.unwrap_or(0));
                    }
                    return Ok(Gv::Ints(v));
                }
                r.seek_relative(sz * n as i64)?;
            } else if et == 8 {
                // Skip arrays of strings (the tokenizer vocabulary).
                for _ in 0..n {
                    let len = rd_u64(r)?;
                    r.seek_relative(len as i64)?;
                }
            } else {
                return Err(bad("nested array"));
            }
            Ok(Gv::Other)
        }
        _ => match rd_int(r, t)? {
            Some(i) => Ok(Gv::Int(i)),
            None => {
                r.seek_relative(scalar_size(t).ok_or_else(|| bad("unknown type"))?)?;
                Ok(Gv::Other)
            }
        },
    }
}

/// Read a model's architecture from its GGUF header.
pub fn read_gguf(path: &Path) -> Result<Arch, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut r = BufReader::with_capacity(1 << 16, f);
    let io = |e: std::io::Error| e.to_string();
    if &rd::<4>(&mut r).map_err(io)? != b"GGUF" {
        return Err("not a GGUF file".into());
    }
    let _version = rd_u32(&mut r).map_err(io)?;
    let _tensors = rd_u64(&mut r).map_err(io)?;
    let kv_count = rd_u64(&mut r).map_err(io)?;
    let mut kv: Vec<(String, Gv)> = Vec::new();
    for _ in 0..kv_count.min(100_000) {
        let key = match rd_value(&mut r, 8).map_err(io)? {
            Gv::Str(s) => s,
            _ => return Err("bad key".into()),
        };
        let t = rd_u32(&mut r).map_err(io)?;
        let v = rd_value(&mut r, t).map_err(io)?;
        if !key.starts_with("tokenizer.") {
            kv.push((key, v));
        }
    }
    arch_from(&kv)
}

fn arch_from(kv: &[(String, Gv)]) -> Result<Arch, String> {
    let get = |k: &str| kv.iter().find(|(key, _)| key == k).map(|(_, v)| v);
    let name = match get("general.architecture") {
        Some(Gv::Str(s)) => s.clone(),
        _ => return Err("no architecture in the GGUF header".into()),
    };
    let int = |suffix: &str| match get(&format!("{name}.{suffix}")) {
        Some(Gv::Int(i)) => Some(*i),
        _ => None,
    };
    let layers = int("block_count").ok_or("no block_count")?;
    let heads = int("attention.head_count").unwrap_or(0);
    let embed = int("embedding_length").unwrap_or(0);
    // head_count_kv may be one number or one per layer (0 = no attention there).
    let (kv_heads, mut attn_layers) = match get(&format!("{name}.attention.head_count_kv")) {
        Some(Gv::Int(i)) => (*i, layers),
        Some(Gv::Ints(v)) => {
            let used: Vec<u64> = v.iter().copied().filter(|h| *h > 0).collect();
            (used.iter().copied().max().unwrap_or(0), used.len() as u64)
        }
        _ => (heads, layers),
    };
    if let Some(interval) = int("full_attention_interval").filter(|i| *i > 1) {
        attn_layers = attn_layers.div_ceil(interval);
    }
    let head_dim = if heads > 0 { embed / heads } else { 128 };
    Ok(Arch {
        key_dim: int("attention.key_length").unwrap_or(head_dim),
        value_dim: int("attention.value_length").unwrap_or(head_dim),
        train_ctx: int("context_length").unwrap_or(0),
        name,
        layers,
        attn_layers,
        kv_heads,
    })
}

/// Bytes per K/V element for a cache type.
pub fn kv_bytes_per_elem(cache: &str) -> f64 {
    match cache {
        "q8_0" => 34.0 / 32.0,
        "q4_0" => 18.0 / 32.0,
        _ => 2.0,
    }
}

/// Recommended load settings for one model on one system.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub context: u64,
    pub gpu_ratio: f64,
    pub gpu_layers: u64,
    pub kv_cache: &'static str,
    pub flash_attention: bool,
    /// "gpu" (all on the GPU), "partial", "cpu" or "too-big".
    pub fit: &'static str,
    pub weights_gb: f64,
    pub kv_gb: f64,
    pub vram_use_gb: f64,
    pub ram_use_gb: f64,
    pub why: Vec<String>,
}

struct Inputs {
    weights: f64,
    kv_elems: f64,
    vram: f64,
    ram: f64,
    max_ctx: u64,
}

impl Inputs {
    fn kv(&self, ctx: u64, cache: &str) -> f64 {
        ctx as f64 * self.kv_elems * kv_bytes_per_elem(cache)
    }
    fn total(&self, ctx: u64, cache: &str) -> f64 {
        self.weights + self.kv(ctx, cache)
    }
    fn fits_gpu(&self, ctx: u64, cache: &str) -> bool {
        self.vram > 0.0 && self.total(ctx, cache) <= self.vram
    }
    fn fits_all(&self, ctx: u64, cache: &str) -> bool {
        self.total(ctx, cache) <= self.vram + self.ram
    }
    fn steps(&self) -> impl Iterator<Item = u64> + '_ {
        let want = self.max_ctx.min(MAX_WANTED_CTX);
        std::iter::once(want).chain(CTX_STEPS.iter().copied().filter(move |c| *c < want))
    }
}

/// Work out how to load a model of `size_bytes` with shape `arch` (if the
/// header could be read) and a maximum context of `max_ctx` on `sys`.
pub fn recommend(sys: &System, size_bytes: u64, arch: Option<&Arch>, max_ctx: u64) -> Plan {
    // Unknown shape: assume a typical 7-14B model (40 layers, 8 KV heads of 128).
    let kv_elems = arch.map(|a| a.kv_elems_per_token()).filter(|n| *n > 0).unwrap_or(40 * 8 * 256) as f64;
    let layers = arch.map(|a| a.layers).filter(|n| *n > 0).unwrap_or(40);
    let vram = sys.gpu.as_ref().map(|g| (g.vram_bytes as f64 - VRAM_RESERVE).max(0.0)).unwrap_or(0.0);
    let ram = (sys.ram_bytes as f64 * 0.8 - RAM_RESERVE).max(sys.ram_bytes as f64 * 0.4);
    let max_ctx = if max_ctx > 0 { max_ctx } else { arch.map(|a| a.train_ctx).filter(|c| *c > 0).unwrap_or(32_768) };
    let inp = Inputs { weights: size_bytes as f64, kv_elems, vram, ram, max_ctx };
    let mut why = Vec::new();
    if arch.is_none() {
        why.push("Could not read the model's layout, so memory use is estimated from typical models.".into());
    }
    let want = max_ctx.min(MAX_WANTED_CTX);
    let floor = COMFORT_CTX.min(want);

    let plan = |ctx: u64, cache: &'static str, ratio: f64, fit: &'static str, why: Vec<String>| {
        let gpu_layers = (ratio * layers as f64).floor() as u64;
        let ratio = if fit == "gpu" { 1.0 } else { gpu_layers as f64 / layers as f64 };
        let total = inp.total(ctx, cache);
        Plan {
            context: ctx,
            gpu_ratio: ratio,
            gpu_layers: if fit == "gpu" { layers } else { gpu_layers },
            kv_cache: cache,
            flash_attention: true,
            fit,
            weights_gb: gb(inp.weights),
            kv_gb: gb(inp.kv(ctx, cache)),
            vram_use_gb: gb(total * ratio),
            ram_use_gb: gb(total * (1.0 - ratio)),
            why,
        }
    };

    // 1. Everything on the GPU at the full wanted context, least quantization first.
    if vram > 0.0 {
        // (q4_0 is not tried here: q8_0 with a somewhat smaller context is
        // the better trade than q4_0 at the maximum.)
        for cache in ["f16", "q8_0"] {
            if inp.fits_gpu(want, cache) {
                why.push(format!("The whole model and a {}-token context fit in your GPU's memory, so it runs at full speed.", kt(want)));
                why.push(cache_reason(cache));
                return plan(want, cache, 1.0, "gpu", why);
            }
        }
        // 2. Still all on the GPU with a smaller (but comfortable) context.
        for cache in ["q8_0", "q4_0"] {
            if let Some(ctx) = inp.steps().find(|c| *c >= floor && inp.fits_gpu(*c, cache)) {
                why.push(format!(
                    "Keeping the whole model on the GPU (fastest) leaves room for {} tokens of context, enough for long pages and a conversation.",
                    kt(ctx)
                ));
                why.push(cache_reason(cache));
                return plan(ctx, cache, 1.0, "gpu", why);
            }
        }
    }
    // 3. Split between GPU and system RAM (or RAM only without a GPU).
    let cache = "q8_0";
    let cap = if vram > 0.0 { want.min(65_536) } else { want.min(16_384) };
    let Some(ctx) = inp.steps().find(|c| *c <= cap && inp.fits_all(*c, cache)).or_else(|| {
        inp.steps().filter(|c| *c <= cap).find(|c| inp.fits_all(*c, "q4_0")).map(|c| c.min(8_192))
    }) else {
        why.push(format!(
            "This model needs about {:.1} GB even with a small context, more than your GPU and RAM can hold. Pick a smaller model.",
            gb(inp.total(8_192, "q4_0"))
        ));
        return plan(8_192, "q4_0", 0.0, "too-big", why);
    };
    let ratio = if vram > 0.0 { (vram / inp.total(ctx, cache)).min(1.0) } else { 0.0 };
    if ratio < 0.05 {
        why.push(if sys.gpu.is_some() {
            "Your GPU's memory is too small for this model, so it runs on the CPU from system RAM. Expect slower replies.".into()
        } else {
            "No dedicated GPU was found, so the model runs on the CPU from system RAM. Expect slower replies; smaller models feel much faster.".into()
        });
        why.push(format!("Context is kept to {} tokens because the CPU reads long prompts slowly.", kt(ctx)));
        why.push(cache_reason(cache));
        return plan(ctx, cache, 0.0, "cpu", why);
    }
    let shown = (ratio * layers as f64).floor() as u64;
    why.push(format!(
        "The model is bigger than your GPU's memory, so {shown} of its {layers} layers go on the GPU and the rest run from system RAM. Replies are slower than fully on the GPU."
    ));
    why.push(format!("Context is set to {} tokens to keep more layers on the GPU.", kt(ctx)));
    why.push(cache_reason(cache));
    plan(ctx, cache, ratio, "partial", why)
}

/// "32k" for 32768, "100k" for 100000.
fn kt(n: u64) -> String {
    if n % 1024 == 0 {
        format!("{}k", n / 1024)
    } else {
        format!("{}k", (n + 500) / 1000)
    }
}

fn cache_reason(cache: &str) -> String {
    match cache {
        "f16" => "K/V cache quantization is off: there is room for the full-precision cache, which is the most accurate.".into(),
        "q8_0" => "K/V cache set to q8_0: it halves the memory the context needs, with almost no loss in quality. Flash attention is turned on because it requires it.".into(),
        _ => "K/V cache set to q4_0: it needs about a quarter of the memory, with a small loss in quality, to fit a useful context. Flash attention is turned on because it requires it.".into(),
    }
}

impl Plan {
    pub fn to_json(&self) -> Value {
        json!({
            "context": self.context, "gpu_ratio": self.gpu_ratio, "gpu_layers": self.gpu_layers,
            "kv_cache": self.kv_cache, "flash_attention": self.flash_attention, "fit": self.fit,
            "weights_gb": self.weights_gb, "kv_gb": self.kv_gb, "vram_use_gb": self.vram_use_gb,
            "ram_use_gb": self.ram_use_gb, "why": self.why,
        })
    }
}

/// Memory budget numbers the settings page uses to recompute its live estimate.
pub fn budget_json(sys: &System) -> Value {
    let vram = sys.gpu.as_ref().map(|g| (g.vram_bytes as f64 - VRAM_RESERVE).max(0.0)).unwrap_or(0.0);
    let ram = (sys.ram_bytes as f64 * 0.8 - RAM_RESERVE).max(sys.ram_bytes as f64 * 0.4);
    json!({"vram_gb": gb(vram), "ram_gb": gb(ram)})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sys(vram_gb: u64, ram_gb: u64) -> System {
        System {
            cpu: "test".into(),
            threads: 16,
            ram_bytes: ram_gb << 30,
            gpu: (vram_gb > 0).then(|| Gpu { name: "Test GPU".into(), vram_bytes: vram_gb << 30, free_bytes: None }),
        }
    }

    fn bonsai() -> Arch {
        // Qwen 3.5 hybrid: 64 layers, attention every 4th.
        Arch { name: "qwen35".into(), layers: 64, attn_layers: 16, kv_heads: 4, key_dim: 256, value_dim: 256, train_ctx: 262_144 }
    }

    fn qwen7b() -> Arch {
        Arch { name: "qwen2".into(), layers: 28, attn_layers: 28, kv_heads: 4, key_dim: 128, value_dim: 128, train_ctx: 32_768 }
    }

    #[test]
    fn hybrid_attention_layers_are_counted() {
        let kv = vec![
            ("general.architecture".to_string(), Gv::Str("qwen35".into())),
            ("qwen35.block_count".into(), Gv::Int(64)),
            ("qwen35.attention.head_count".into(), Gv::Int(24)),
            ("qwen35.attention.head_count_kv".into(), Gv::Int(4)),
            ("qwen35.attention.key_length".into(), Gv::Int(256)),
            ("qwen35.attention.value_length".into(), Gv::Int(256)),
            ("qwen35.embedding_length".into(), Gv::Int(5120)),
            ("qwen35.full_attention_interval".into(), Gv::Int(4)),
            ("qwen35.context_length".into(), Gv::Int(262_144)),
        ];
        assert_eq!(arch_from(&kv).unwrap(), bonsai());
    }

    #[test]
    fn head_dim_defaults_from_embedding() {
        let kv = vec![
            ("general.architecture".to_string(), Gv::Str("qwen2".into())),
            ("qwen2.block_count".into(), Gv::Int(28)),
            ("qwen2.attention.head_count".into(), Gv::Int(28)),
            ("qwen2.attention.head_count_kv".into(), Gv::Int(4)),
            ("qwen2.embedding_length".into(), Gv::Int(3584)),
            ("qwen2.context_length".into(), Gv::Int(32_768)),
        ];
        assert_eq!(arch_from(&kv).unwrap(), qwen7b());
    }

    #[test]
    fn small_model_runs_fully_on_a_big_gpu_without_quantization() {
        let p = recommend(&sys(12, 32), 4_683_074_048, Some(&qwen7b()), 32_768);
        assert_eq!((p.fit, p.kv_cache, p.context, p.gpu_ratio), ("gpu", "f16", 32_768, 1.0));
    }

    #[test]
    fn bonsai_on_12gb_quantizes_the_cache_to_stay_on_gpu() {
        let p = recommend(&sys(12, 32), 7_206_168_928, Some(&bonsai()), 262_144);
        assert_eq!(p.fit, "gpu");
        assert_ne!(p.kv_cache, "f16");
        assert!(p.context >= COMFORT_CTX, "{p:?}");
    }

    #[test]
    fn bonsai_on_12gb_prefers_q8_over_q4() {
        let p = recommend(&sys(12, 32), 7_206_168_928, Some(&bonsai()), 262_144);
        assert_eq!((p.kv_cache, p.context), ("q8_0", 100_000), "{p:?}");
    }

    #[test]
    fn bonsai_on_8gb_splits_between_gpu_and_ram() {
        let p = recommend(&sys(8, 32), 7_206_168_928, Some(&bonsai()), 262_144);
        assert_eq!(p.fit, "partial");
        assert!(p.gpu_ratio > 0.3 && p.gpu_ratio < 1.0, "{p:?}");
        assert!(p.gpu_layers < 64);
    }

    #[test]
    fn no_gpu_means_cpu_with_a_modest_context() {
        let p = recommend(&sys(0, 16), 4_683_074_048, Some(&qwen7b()), 32_768);
        assert_eq!(p.fit, "cpu");
        assert_eq!(p.gpu_ratio, 0.0);
        assert!(p.context <= 16_384);
    }

    #[test]
    fn huge_model_is_too_big() {
        let p = recommend(&sys(4, 8), 60 << 30, Some(&qwen7b()), 32_768);
        assert_eq!(p.fit, "too-big");
    }

    #[test]
    fn integrated_graphics_are_not_a_gpu() {
        assert!(is_integrated("Intel(R) Graphics"));
        assert!(is_integrated("Intel(R) UHD Graphics 770"));
        assert!(!is_integrated("Intel(R) Arc(TM) A770 Graphics"));
        assert!(!is_integrated("Intel(R) Arc(TM) B580 Graphics"));
        assert!(is_integrated("Intel(R) Arc(TM) Graphics"));
        assert!(!is_integrated("AMD Radeon RX 7800 XT"));
        assert!(is_integrated("AMD Radeon(TM) Graphics"));
        assert!(is_integrated("MuMuPlayer Virtual Display Adapter"));
    }
}
