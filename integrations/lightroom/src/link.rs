//! Link to the Lightroom Classic plugin (lightroom/README.md): the plugin listens on
//! 58781 (plugin -> helper) and 58782 (helper -> plugin); we connect to both, one JSON per line.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const P2H_PORT: u16 = 58781;
pub const H2P_PORT: u16 = 58782;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CatItem { pub id: String, pub label: String, pub group: String }

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct LrState {
    pub connected: bool,
    pub module: Option<String>,
    pub tool: Option<String>,
    pub photo: bool,
    pub lr_version: Option<String>,
    pub plugin: Option<String>,
    /// from the plugin's "state" messages (LED rules)
    pub flag: Option<String>,
    pub rating: Option<f64>,
    pub label: Option<String>,
    pub cg_view: Option<String>,
    pub values: std::collections::HashMap<String, f64>,
    #[serde(skip)] pub params: Vec<CatItem>,
    #[serde(skip)] pub cmds: Vec<CatItem>,
    pub errors: u64,
    /// counts connections, so a new one gets the watch list again
    #[serde(skip)] pub connection: u64,
}

/// The plugin's catalogue from the last connection, so the editor can name settings and commands
/// while Lightroom is closed.
#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct Catalogue { params: Vec<CatItem>, cmds: Vec<CatItem> }

const CATALOGUE_FILE: &str = "lightroom_catalogue.json";
/// The list as this version's plugin sends it, for before the first connection. Made by tests/test_lua_catalog.py --write.
const BUILT_IN: &str = include_str!("catalogue.json");

fn read_catalogue(p: &std::path::Path) -> Option<Catalogue> {
    let t = match std::fs::read_to_string(p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => { micropanel::util::log(&format!("lightroom: can't read {}: {e}", p.display())); return None; }
    };
    serde_json::from_str(&t).map_err(|e| micropanel::util::log(&format!("lightroom: {} is not usable: {e}", p.display()))).ok()
}

fn write_catalogue(p: &std::path::Path, c: &Catalogue) {
    if c.params.is_empty() || read_catalogue(p).as_ref() == Some(c) { return; }
    let r = serde_json::to_string(c).map_err(|e| e.to_string()).and_then(|t| std::fs::write(p, t).map_err(|e| e.to_string()));
    if let Err(e) = r { micropanel::util::log(&format!("lightroom: can't save {}: {e}", p.display())); }
}

impl LrState {
    /// Not connected, with the catalogue of the last connection, or the built-in one.
    pub fn remembered() -> LrState {
        let c = read_catalogue(&micropanel::util::config_dir().join(CATALOGUE_FILE)).filter(|c| !c.params.is_empty())
            .unwrap_or_else(|| serde_json::from_str(BUILT_IN).expect("the built-in catalogue"));
        LrState { params: c.params, cmds: c.cmds, ..Default::default() }
    }
}

pub struct LrLink {
    pub state: Arc<Mutex<LrState>>,
    tx: Arc<Mutex<Option<TcpStream>>>,
    pub sent: Arc<Mutex<u64>>,
}

/// The sending half, for a thread of its own.
#[derive(Clone)]
pub struct LrSender { tx: Arc<Mutex<Option<TcpStream>>>, sent: Arc<Mutex<u64>>, state: Arc<Mutex<LrState>> }
impl LrSender {
    /// Only to a plugin that knows this message.
    pub fn send(&self, msg: &Value) -> bool {
        { let s = self.state.lock().unwrap(); if !s.connected || !s.plugin.as_deref().is_some_and(|p| crate::newer(p, [0, 3, 3])) { return false; } }
        LrLink { state: self.state.clone(), tx: self.tx.clone(), sent: self.sent.clone() }.send(msg)
    }
}

impl LrLink {
    pub fn sender(&self) -> LrSender { LrSender { tx: self.tx.clone(), sent: self.sent.clone(), state: self.state.clone() } }

    pub fn new() -> LrLink {
        LrLink { state: Arc::new(Mutex::new(LrState::remembered())), tx: Arc::default(), sent: Arc::default() }
    }

    pub fn start(&self) {
        let (state, tx) = (self.state.clone(), self.tx.clone());
        std::thread::Builder::new().name("lrlink".into()).spawn(move || run(state, tx)).unwrap();
    }

    /// Send one message; false if not connected (or the write failed).
    pub fn send(&self, msg: &Value) -> bool {
        let mut g = self.tx.lock().unwrap();
        let Some(s) = g.as_mut() else { return false; };
        let line = format!("{msg}\n");
        match s.write_all(line.as_bytes()) {
            Ok(()) => { *self.sent.lock().unwrap() += 1; true }
            Err(e) => {
                micropanel::util::log(&format!("lr send failed: {e}"));
                let _ = s.shutdown(Shutdown::Both);
                *g = None;
                false
            }
        }
    }
}

fn connect(port: u16) -> Option<TcpStream> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let s = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    let _ = s.set_nodelay(true);
    Some(s)
}

fn run(state: Arc<Mutex<LrState>>, tx: Arc<Mutex<Option<TcpStream>>>) {
    let mut logged_wait = false;
    loop {
        // Both sockets, or neither.
        let (Some(rx), Some(txs)) = (connect(P2H_PORT), connect(H2P_PORT)) else {
            if !logged_wait { micropanel::util::log("lightroom: waiting for the plugin (ports 58781/58782)"); logged_wait = true; }
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        logged_wait = false;
        micropanel::util::log("lightroom: connected to the plugin");
        // Lightroom can stop reading for a while (a modal dialog, a plugin reload): never block on a
        // write, or the whole helper waits with it. A timed-out write drops the link; it reconnects.
        let _ = txs.set_write_timeout(Some(Duration::from_millis(300)));
        *tx.lock().unwrap() = Some(txs);
        {
            let mut st = state.lock().unwrap();
            let (params, cmds) = (std::mem::take(&mut st.params), std::mem::take(&mut st.cmds));
            let connection = st.connection + 1;
            *st = LrState { connected: true, params, cmds, connection, ..Default::default() };
        }
        let send = |m: Value| {
            let mut g = tx.lock().unwrap();
            if let Some(s) = g.as_mut() { let _ = s.write_all(format!("{m}\n").as_bytes()); }
        };
        send(json!({"t":"hello","v":1,"helper":env!("CARGO_PKG_VERSION")}));

        let _ = rx.set_read_timeout(Some(Duration::from_secs(1)));
        let mut reader = BufReader::new(rx);
        let mut last_pong = Instant::now();
        let mut last_ping = Instant::now();
        let mut line = String::new();
        let mut new_cat: Vec<(String, CatItem)> = vec![];
        let t_connect = Instant::now();
        let mut got_hello = false;
        loop {
            // A stale plugin send socket accepts the TCP connection but never answers: retry.
            if !got_hello && t_connect.elapsed() >= Duration::from_secs(3) {
                micropanel::util::log("lightroom: no hello from the plugin within 3 s; reconnecting");
                break;
            }
            if last_ping.elapsed() >= Duration::from_secs(5) {
                send(json!({"t":"ping"}));
                last_ping = Instant::now();
            }
            if last_pong.elapsed() >= Duration::from_secs(15) {
                micropanel::util::log("lightroom: no pong for 15 s; reconnecting");
                break;
            }
            if tx.lock().unwrap().is_none() { break; }
            // a timeout keeps the partial line in line; it is cleared only after a full line
            match reader.read_line(&mut line) {
                Ok(0) => { micropanel::util::log("lightroom: plugin closed the connection"); break; }
                Ok(_) => {
                    let text = std::mem::take(&mut line);
                    let Ok(v) = serde_json::from_str::<Value>(text.trim()) else {
                        micropanel::util::log(&format!("lightroom: bad line {:?}", text.trim()));
                        continue;
                    };
                    last_pong = Instant::now(); // any traffic counts as alive
                    let mut st = state.lock().unwrap();
                    match v["t"].as_str().unwrap_or("") {
                        "hello" => { got_hello = true; st.plugin = v["plugin"].as_str().map(String::from); st.lr_version = v["lr"].as_str().map(String::from); }
                        "pong" => {}
                        "context" => {
                            st.module = v["module"].as_str().map(String::from);
                            st.tool = v["tool"].as_str().map(String::from);
                            st.photo = v["photo"].as_bool().unwrap_or(false);
                        }
                        "state" => {
                            st.flag = v["flag"].as_str().map(String::from);
                            st.rating = v["rating"].as_f64();
                            st.label = v["label"].as_str().map(String::from);
                            st.cg_view = v["cg_view"].as_str().map(String::from);
                            st.values = v["values"].as_object().map(|o| o.iter()
                                .filter_map(|(k, x)| x.as_f64().map(|f| (k.clone(), f))).collect()).unwrap_or_default();
                        }
                        "catalog" => new_cat.push((v["kind"].as_str().unwrap_or("").to_string(), CatItem {
                            id: v["id"].as_str().unwrap_or("").into(),
                            label: v["label"].as_str().unwrap_or("").into(),
                            group: v["group"].as_str().unwrap_or("").into(),
                        })),
                        "catalog_end" => {
                            st.params = new_cat.iter().filter(|(k, _)| k == "param").map(|(_, c)| c.clone()).collect();
                            st.cmds = new_cat.iter().filter(|(k, _)| k == "cmd").map(|(_, c)| c.clone()).collect();
                            micropanel::util::log(&format!("lightroom: catalogue {} settings, {} commands (plugin says {})",
                                st.params.len(), st.cmds.len(), v["n"]));
                            new_cat.clear();
                            let c = Catalogue { params: st.params.clone(), cmds: st.cmds.clone() };
                            drop(st);
                            write_catalogue(&micropanel::util::config_dir().join(CATALOGUE_FILE), &c);
                        }
                        "error" => { st.errors += 1; micropanel::util::log(&format!("lightroom plugin error: {}", v["msg"])); }
                        other => micropanel::util::log(&format!("lightroom: unknown message type {other:?}")),
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => { micropanel::util::log(&format!("lightroom: read failed: {e}")); break; }
            }
        }
        if let Some(s) = tx.lock().unwrap().take() { let _ = s.shutdown(Shutdown::Both); }
        let _ = reader.get_ref().shutdown(Shutdown::Both);
        state.lock().unwrap().connected = false;
        std::thread::sleep(Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_catalogue() {
        let c: Catalogue = serde_json::from_str(BUILT_IN).unwrap();
        assert!(c.params.iter().any(|p| p.id == "Exposure2012" && p.label == "Exposure") && c.cmds.iter().any(|p| p.id == "undo"));
    }

    #[test]
    fn catalogue_is_remembered() {
        let p = std::env::temp_dir().join(format!("mp_cat_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        assert!(read_catalogue(&p).is_none());
        write_catalogue(&p, &Catalogue::default());
        assert!(!p.exists(), "an empty catalogue never replaces a saved one");
        let it = |i: &str| CatItem { id: i.into(), label: i.to_uppercase(), group: "G".into() };
        let c = Catalogue { params: vec![it("a")], cmds: vec![it("b")] };
        write_catalogue(&p, &c);
        assert_eq!(read_catalogue(&p), Some(c));
        std::fs::write(&p, "not json").unwrap();
        assert!(read_catalogue(&p).is_none());
        let _ = std::fs::remove_file(&p);
    }
}
