use serde::{Deserialize, Serialize};

/// Messages sent from makod (host) to mako-agent (guest) over the vsock control channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum HostMessage {
    Ping,
    GetStatus,
    StartDocker,
    StopDocker,
    RestartDocker,
    Shutdown,
}

/// Messages sent from mako-agent (guest) to makod (host) over the vsock control channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum AgentMessage {
    Pong,
    Status(AgentStatus),
    Ack,
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatus {
    pub dockerd_running: bool,
    pub containerd_running: bool,
    pub uptime_seconds: u64,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub cpu_usage_percent: f32,
}

/// Well-known vsock CID for the host.
pub const VSOCK_HOST_CID: u32 = 2;
/// vsock CID for the guest (assigned by Virtualization.framework).
pub const VSOCK_GUEST_CID: u32 = 3;
