//! A tiny extension system for `jj1-browser`.
//!
//! An extension is a folder in `%APPDATA%\jj1\extensions\<id>\` with:
//!   - `manifest.json`  { "name", "description", "version", "enabled" }
//!   - `content.js`     JavaScript injected into every page, at document start.
//!
//! Content scripts run in the page and may call a small `jj1` API (see
//! `API_BRIDGE`): `jj1.captureTab()`, `jj1.notify(text)`, `jj1.log(text)`.
//! Extensions are the user's own code in their own profile, so they are
//! trusted; the bridge is only injected when at least one is enabled.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Extension {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub enabled: bool,
    pub content_js: String,
}

pub fn dir(config_dir: &Path) -> PathBuf {
    config_dir.join("extensions")
}

/// The `jj1` object every content script can use. Injected once, before the
/// content scripts. `notify` is purely in-page; `captureTab` asks the browser.
pub const API_BRIDGE: &str = r#"(function(){
  if (window.jj1 && window.jj1.__ready) return;
  function ipc(m){ try{ if(window.ipc) window.ipc.postMessage(m); }catch(e){} }
  window.jj1 = {
    __ready: true,
    version: "1",
    captureTab: function(){ ipc('ext:capture'); },
    notify: function(msg){
      try{
        var t=document.createElement('div');
        t.textContent=String(msg);
        t.style.cssText='position:fixed;z-index:2147483647;left:50%;bottom:26px;transform:translateX(-50%);'
          +'background:rgba(24,26,30,.86);color:#fff;padding:11px 20px;border-radius:14px;'
          +'font:14px/1.3 system-ui,"Segoe UI",sans-serif;box-shadow:0 10px 36px rgba(0,0,0,.4);'
          +'backdrop-filter:blur(8px);-webkit-backdrop-filter:blur(8px);max-width:80vw;pointer-events:none;opacity:0;'
          +'transition:opacity .18s ease,transform .18s ease';
        (document.body||document.documentElement).appendChild(t);
        requestAnimationFrame(function(){ t.style.opacity='1'; t.style.transform='translateX(-50%) translateY(-4px)'; });
        setTimeout(function(){ t.style.opacity='0'; setTimeout(function(){ t.remove(); }, 250); }, 2600);
      }catch(e){}
    },
    log: function(m){ try{ console.log('[jj1 ext]', m); }catch(e){} }
  };
})();"#;

fn read_str(v: &serde_json::Value, key: &str, default: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or(default).to_string()
}

/// Load every extension in the profile, enabled or not (callers filter).
/// A folder without a readable `content.js` is skipped.
pub fn load_all(config_dir: &Path) -> Vec<Extension> {
    let root = dir(config_dir);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        let content_js = match std::fs::read_to_string(path.join("content.js")) {
            Ok(js) => js,
            Err(_) => continue,
        };
        let manifest = std::fs::read_to_string(path.join("manifest.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .unwrap_or(serde_json::Value::Null);
        out.push(Extension {
            name: read_str(&manifest, "name", &id),
            description: read_str(&manifest, "description", ""),
            version: read_str(&manifest, "version", "0"),
            enabled: manifest.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true),
            content_js,
            id,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// One script to inject at document start: the API bridge, then each enabled
/// extension's content script in its own sandboxed IIFE so one failing
/// extension cannot break another or the page.
pub fn combined_script(exts: &[Extension]) -> String {
    let enabled: Vec<&Extension> = exts.iter().filter(|e| e.enabled).collect();
    if enabled.is_empty() {
        return String::new();
    }
    let mut s = String::from(API_BRIDGE);
    for e in enabled {
        s.push_str("\n;(function(){ try {\n");
        s.push_str(&e.content_js);
        s.push_str(&format!("\n}} catch (err) {{ console.error('[jj1 ext {}]', err); }} }})();\n", e.id.replace('\'', "")));
    }
    s
}

/// Write the built-in "SS screenshot" extension the first time, so the folder
/// is never empty and there is a worked example to copy. Does nothing if the
/// extensions folder already exists.
pub fn ensure_samples(config_dir: &Path) {
    let root = dir(config_dir);
    if root.exists() {
        return;
    }
    let ext = root.join("ss-screenshot");
    if std::fs::create_dir_all(&ext).is_err() {
        return;
    }
    let manifest = serde_json::json!({
        "name": "SS Screenshot",
        "description": "Press S twice quickly (\"SS\") on any page to save a full screenshot of the tab.",
        "version": "1.0",
        "enabled": true
    });
    let _ = std::fs::write(ext.join("manifest.json"), serde_json::to_string_pretty(&manifest).unwrap_or_default());
    let _ = std::fs::write(ext.join("content.js"), SS_SCREENSHOT_JS);
    // A short readme so the folder explains itself.
    let _ = std::fs::write(root.join("README.txt"), EXTENSIONS_README);
}

const SS_SCREENSHOT_JS: &str = r#"// SS Screenshot - press "S" twice quickly to screenshot the whole tab.
// A worked example of a JJ1 extension. Edit freely; reload the browser to apply.
(function () {
  var last = 0;
  var GAP = 500; // ms between the two S presses

  function typing() {
    var el = document.activeElement;
    return el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable);
  }

  window.addEventListener('keydown', function (e) {
    if (e.ctrlKey || e.metaKey || e.altKey || typing()) { last = 0; return; }
    if (e.key === 's' || e.key === 'S') {
      var now = Date.now();
      if (now - last < GAP) {
        last = 0;
        jj1.captureTab();          // JJ1 saves the shot and shows the result
      } else {
        last = now;
      }
    } else {
      last = 0;
    }
  }, true);
})();
"#;

const EXTENSIONS_README: &str = "JJ1 extensions\n\
==============\n\n\
Each folder here is one extension:\n\
  <folder>/manifest.json   name, description, version, enabled (true/false)\n\
  <folder>/content.js      JavaScript injected into every page you visit\n\n\
Your content.js can use the jj1 API:\n\
  jj1.captureTab()     save a PNG of the current tab to Pictures\\JJ1\n\
  jj1.notify(text)     show a short toast on the page\n\
  jj1.log(text)        write to the page console\n\n\
Reload the browser (or open a new window) after changing an extension.\n\
See ss-screenshot for a worked example.\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combined_script_wraps_each_extension() {
        let exts = vec![
            Extension { id: "a".into(), name: "A".into(), description: String::new(), version: "1".into(), enabled: true, content_js: "doA();".into() },
            Extension { id: "b".into(), name: "B".into(), description: String::new(), version: "1".into(), enabled: false, content_js: "doB();".into() },
        ];
        let s = combined_script(&exts);
        assert!(s.contains("window.jj1"));
        assert!(s.contains("doA();"));
        assert!(!s.contains("doB();"), "disabled extensions are not injected");
        assert!(s.contains("[jj1 ext a]"));
    }

    #[test]
    fn no_extensions_means_no_script() {
        assert_eq!(combined_script(&[]), "");
        let disabled = vec![Extension { id: "x".into(), name: "X".into(), description: String::new(), version: "1".into(), enabled: false, content_js: "x();".into() }];
        assert_eq!(combined_script(&disabled), "");
    }

    #[test]
    fn sample_is_written_once() {
        let tmp = std::env::temp_dir().join(format!("jj1-ext-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        ensure_samples(&tmp);
        let exts = load_all(&tmp);
        assert_eq!(exts.len(), 1);
        assert_eq!(exts[0].id, "ss-screenshot");
        assert!(exts[0].enabled);
        assert!(exts[0].content_js.contains("captureTab"));
        // Second call does not duplicate or overwrite.
        std::fs::write(dir(&tmp).join("ss-screenshot").join("content.js"), "edited();").unwrap();
        ensure_samples(&tmp);
        assert_eq!(load_all(&tmp)[0].content_js, "edited();");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
