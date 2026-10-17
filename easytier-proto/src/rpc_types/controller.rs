use std::sync::{Arc, Mutex};

use bytes::Bytes;

use crate::proto::common::TunnelInfo;

// Controller must impl clone and all cloned controllers share the same data
pub trait Controller: Send + Sync + Clone + 'static {
    fn timeout_ms(&self) -> i32 {
        5000
    }

    fn set_timeout_ms(&mut self, _timeout_ms: i32) {}

    fn set_trace_id(&mut self, _trace_id: i32) {}

    fn trace_id(&self) -> i32 {
        0
    }

    fn set_raw_input(&mut self, _raw_input: Bytes) {}
    fn get_raw_input(&self) -> Option<Bytes> {
        None
    }

    fn set_tunnel_info(&mut self, _tunnel_info: Option<TunnelInfo>) {}
    fn get_tunnel_info(&self) -> Option<&TunnelInfo> {
        None
    }

    fn set_raw_output(&mut self, _raw_output: Bytes) {}
    fn get_raw_output(&self) -> Option<Bytes> {
        None
    }
}

#[derive(Debug)]
pub struct BaseControllerRawData {
    pub raw_input: Option<Bytes>,
    pub raw_output: Option<Bytes>,
}

#[derive(Debug, Clone)]
pub struct BaseController {
    pub timeout_ms: i32,
    pub trace_id: i32,
    pub raw_data: Arc<Mutex<BaseControllerRawData>>,
    pub tunnel_info: Option<TunnelInfo>,
    /// Peer id **verified by the transport layer** for the tunnel this call
    /// arrived on, filled in by the server-side dispatcher only.
    ///
    /// CRITICAL (CR-07): `PeerConn` already drops any packet whose
    /// `from_peer_id` disagrees with the authenticated peer id, so this value is
    /// trustworthy. Handlers must prefer it over any peer id inside the request
    /// body — `OspfRouteRpc::sync_route_info` used to read `request.my_peer_id`
    /// directly, which let any member act as (and inject routes in the name of)
    /// any other peer, and made `get_peer_identity_type_from_interface` return
    /// `None` for the impersonated id, which in turn defaulted it to `Admin` and
    /// bypassed the credential restrictions entirely.
    ///
    /// `None` on the **client** side (there is no inbound transport) and in unit
    /// tests that construct a controller by hand; handlers that require identity
    /// binding must reject `None` rather than falling back to the request body.
    ///
    /// This field is purely in-memory — `BaseController` is not a protobuf
    /// message and is never serialized — so a peer can neither see nor forge it.
    pub verified_from_peer_id: Option<u32>,
}

impl Controller for BaseController {
    fn timeout_ms(&self) -> i32 {
        self.timeout_ms
    }

    fn set_timeout_ms(&mut self, timeout_ms: i32) {
        self.timeout_ms = timeout_ms;
    }

    fn set_trace_id(&mut self, trace_id: i32) {
        self.trace_id = trace_id;
    }

    fn trace_id(&self) -> i32 {
        self.trace_id
    }

    fn set_raw_input(&mut self, raw_input: Bytes) {
        self.raw_data.lock().unwrap().raw_input = Some(raw_input);
    }

    fn get_raw_input(&self) -> Option<Bytes> {
        self.raw_data.lock().unwrap().raw_input.clone()
    }

    fn set_raw_output(&mut self, raw_output: Bytes) {
        self.raw_data.lock().unwrap().raw_output = Some(raw_output);
    }

    fn get_raw_output(&self) -> Option<Bytes> {
        self.raw_data.lock().unwrap().raw_output.clone()
    }

    fn get_tunnel_info(&self) -> Option<&TunnelInfo> {
        self.tunnel_info.as_ref()
    }

    fn set_tunnel_info(&mut self, tunnel_info: Option<TunnelInfo>) {
        self.tunnel_info = tunnel_info;
    }
}

impl Default for BaseController {
    fn default() -> Self {
        Self {
            timeout_ms: 5000,
            trace_id: 0,
            raw_data: Arc::new(Mutex::new(BaseControllerRawData {
                raw_input: None,
                raw_output: None,
            })),
            tunnel_info: None,
            verified_from_peer_id: None,
        }
    }
}
