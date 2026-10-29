//! The request surface shared by every front end (the Tauri app on each platform).
//! Method names and JSON shapes are what the UI's library interface calls.

use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::Value;

use crate::Node;

#[derive(Deserialize)]
struct TicketParams {
    ticket: String,
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct SettingsParams {
    patch: serde_json::Map<String, Value>,
}

pub async fn dispatch(node: &Node, method: &str, params: Value) -> Result<Value> {
    let p = params;
    Ok(match method {
        "status" => serde_json::to_value(node.status())?,
        "outgoing" => serde_json::to_value(node.outgoing())?,
        "settings" => Value::Object(node.settings()),
        "setSettings" => {
            let p: SettingsParams = serde_json::from_value(p)?;
            Value::Object(node.set_settings(p.patch).await?)
        }
        "createInvite" => serde_json::to_value(node.create_invite()?)?,
        "join" => {
            let p: TicketParams = serde_json::from_value(p)?;
            serde_json::to_value(node.use_code(&p.ticket).await?)?
        }
        "networkChanged" => {
            node.network_changed().await;
            Value::Null
        }
        "removeDevice" => {
            let p: IdParams = serde_json::from_value(p)?;
            node.remove_device(p.id).await?;
            Value::Null
        }
        other => bail!("unknown method {other}"),
    })
}
