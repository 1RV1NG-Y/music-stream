//! Single owner for the Android node. Readers borrow a short-lived clone for each command;
//! neither Tauri's managed state nor the service retains one across stop/start.
use std::sync::Mutex;
use musicstream_core::{Node, NodeConfig};

pub struct NodeSlot(Mutex<Option<Node>>);

impl NodeSlot {
    pub const fn new() -> Self { Self(Mutex::new(None)) }

    pub fn node(&self) -> Result<Node, String> {
        self.0.lock().map_err(|e| e.to_string())?.clone()
            .ok_or_else(|| "Serving is paused. Reopen music-stream to resume.".into())
    }

    pub fn start(&self, config: NodeConfig) -> Result<(Node, bool), String> {
        let mut slot = self.0.lock().map_err(|e| e.to_string())?;
        if let Some(node) = slot.as_ref() { return Ok((node.clone(), false)); }
        let node = tauri::async_runtime::block_on(Node::open(config)).map_err(|e| format!("{e:#}"))?;
        *slot = Some(node.clone());
        Ok((node, true))
    }

    pub fn stop(&self) -> Result<(), String> {
        let mut slot = self.0.lock().map_err(|e| e.to_string())?;
        if let Some(node) = slot.take() {
            tauri::async_runtime::block_on(node.shutdown());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_pause_reopen_preserves_identity_and_releases_library_lock() {
        let dir = std::env::temp_dir().join(format!("music-stream-node-slot-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let slot = NodeSlot::new();
        let config = || NodeConfig { data_dir: dir.clone(), device_name: "Android lifecycle test".into(),
            platform: "android".into(), port: 0, mdns: false };
        let mut identity = None;
        for _ in 0..3 {
            let (node, created) = slot.start(config()).unwrap();
            assert!(created);
            let id = node.status().device_id;
            assert_eq!(identity.get_or_insert(id.clone()), &id);
            let (second, created) = slot.start(config()).unwrap();
            assert!(!created, "activity attach must share the service node");
            assert_eq!(second.status().device_id, id);
            // An event subscription (like the WebView forwarder) must not retain the node.
            let _events = node.subscribe();
            drop(second);
            drop(node);
            slot.stop().unwrap();
            slot.stop().unwrap();
            assert!(slot.node().is_err());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
