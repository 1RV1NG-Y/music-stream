//! music-stream core: device identity, pairing, discovery and a Loro-replicated document of
//! user data, synced between paired devices over iroh (LAN discovery via mDNS). Forked from
//! the reader-sync core (`1RV1NG-Y/reader-sync` `4fab02e`), itself forked from Bits; fixes
//! are ported by hand (PLAN.md §2).

pub mod model;
mod node;
pub mod proto;
pub mod rpc;
pub mod store;

pub use iroh::EndpointId;
pub use node::{DeviceStatus, Event, Invite, Node, NodeConfig, Outgoing, PairResult, Status};

/// UDP port devices listen on (Bits uses 47100 and reader-sync 47110, so all can run at once).
pub const DEFAULT_PORT: u16 = 47120;
