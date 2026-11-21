//! UPnP Internet Gateway Device: SSDP discovery and the three SOAP actions
//! Hearthgate needs (add and delete a port mapping, read the public IP).
//!
//! Routers are untrusted: their answers are size-limited, read with plain
//! string searches rather than a full XML parser, and the control URL must
//! point at the same address that answered the discovery.

use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tokio::net::UdpSocket;

use crate::{Error, Protocol};

/// The SSDP multicast address.
pub const SSDP: &str = "239.255.255.250:1900";
const MAX_BODY: usize = 256 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

const SEARCH_TARGETS: &[&str] = &[
    "urn:schemas-upnp-org:device:InternetGatewayDevice:1",
    "urn:schemas-upnp-org:device:InternetGatewayDevice:2",
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
];

/// A router found by discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gateway {
    /// Address that answered the search.
    pub addr: SocketAddr,
    /// SOAP control endpoint of the WAN connection service.
    pub control_url: String,
    /// Full service type, for example `urn:schemas-upnp-org:service:WANIPConnection:1`.
    pub service_type: String,
    /// The router's own name, when it gives one.
    pub name: Option<String>,
}

/// Searches for a gateway. `target` is normally [`SSDP`]; tests point it
/// at a local responder.
pub async fn discover(target: SocketAddr, wait: Duration) -> Result<Gateway, Error> {
    let bind: SocketAddr = if target.is_ipv4() {
        "0.0.0.0:0".parse().expect("valid")
    } else {
        "[::]:0".parse().expect("valid")
    };
    let socket = UdpSocket::bind(bind).await?;
    for st in SEARCH_TARGETS {
        let msg = format!(
            "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {st}\r\n\r\n"
        );
        socket.send_to(msg.as_bytes(), target).await?;
    }
    let deadline = tokio::time::Instant::now() + wait;
    let mut buf = vec![0u8; 2048];
    let mut last_error = Error::NotFound;
    loop {
        let recv = tokio::time::timeout_at(deadline, socket.recv_from(&mut buf)).await;
        let Ok(Ok((n, from))) = recv else {
            return Err(last_error);
        };
        let text = String::from_utf8_lossy(&buf[..n]);
        let Some(location) = header(&text, "location") else {
            continue;
        };
        match describe(from, location).await {
            Ok(gw) => return Ok(gw),
            Err(e) => last_error = e,
        }
    }
}

/// A header value from an SSDP response, case-insensitively.
fn header<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

fn client() -> Client<HttpConnector, Full<Bytes>> {
    let mut connector = HttpConnector::new();
    connector.set_connect_timeout(Some(REQUEST_TIMEOUT));
    Client::builder(TokioExecutor::new()).build(connector)
}

async fn http(
    method: http::Method,
    url: &str,
    headers: &[(&str, String)],
    body: String,
) -> Result<(http::StatusCode, String), Error> {
    let mut req = http::Request::builder().method(method).uri(url);
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let req = req
        .body(Full::new(Bytes::from(body)))
        .map_err(|e| Error::Router(e.to_string()))?;
    let send = async {
        let resp = client()
            .request(req)
            .await
            .map_err(|e| Error::Router(format!("{e}")))?;
        let status = resp.status();
        let body = Limited::new(resp.into_body(), MAX_BODY)
            .collect()
            .await
            .map_err(|e| Error::Router(format!("{e}")))?
            .to_bytes();
        Ok::<_, Error>((status, String::from_utf8_lossy(&body).into_owned()))
    };
    tokio::time::timeout(REQUEST_TIMEOUT, send)
        .await
        .map_err(|_| Error::Router("the router did not answer in time".into()))?
}

/// Reads the device description and finds the WAN connection service.
async fn describe(from: SocketAddr, location: &str) -> Result<Gateway, Error> {
    let uri: http::Uri = location
        .parse()
        .map_err(|_| Error::Router(format!("bad location {location}")))?;
    let host_ip = uri
        .host()
        .and_then(|h| h.trim_matches(['[', ']']).parse::<IpAddr>().ok());
    if uri.scheme_str() != Some("http") || host_ip != Some(from.ip()) {
        // A description elsewhere could make us talk to any host.
        return Err(Error::Router(format!(
            "ignoring {location}: it is not on the address that answered ({})",
            from.ip()
        )));
    }
    let (status, xml) = http(http::Method::GET, location, &[], String::new()).await?;
    if !status.is_success() {
        return Err(Error::Router(format!("device description: {status}")));
    }
    let base = format!(
        "http://{}",
        uri.authority().map_or("", http::uri::Authority::as_str)
    );
    let name = tag(&xml, "friendlyName").map(str::to_owned);
    for service in blocks(&xml, "service") {
        let Some(service_type) = tag(service, "serviceType") else {
            continue;
        };
        if !(service_type.contains(":WANIPConnection:")
            || service_type.contains(":WANPPPConnection:"))
        {
            continue;
        }
        let Some(control) = tag(service, "controlURL") else {
            continue;
        };
        let control_url = if control.starts_with("http://") {
            control.to_owned()
        } else {
            format!("{base}/{}", control.trim_start_matches('/'))
        };
        let control_host = control_url
            .parse::<http::Uri>()
            .ok()
            .and_then(|u| u.host().map(|h| h.trim_matches(['[', ']']).to_owned()));
        if control_host.and_then(|h| h.parse::<IpAddr>().ok()) != Some(from.ip()) {
            continue;
        }
        return Ok(Gateway {
            addr: from,
            control_url,
            service_type: service_type.to_owned(),
            name,
        });
    }
    Err(Error::Router(
        "the router has no WAN connection service".into(),
    ))
}

/// The text of the first `<name>…</name>` (namespace prefixes allowed).
pub(crate) fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = find_open(xml, name)?;
    let rest = &xml[open..];
    let end = find_close(rest, name)?;
    Some(xml_unescape_trim(&rest[..end]))
}

fn find_open(xml: &str, name: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = xml[from..].find('<') {
        let start = from + i + 1;
        let rest = &xml[start..];
        let end = rest.find('>')?;
        let tag_text = &rest[..end];
        let local = tag_text
            .split_whitespace()
            .next()
            .unwrap_or("")
            .rsplit(':')
            .next()
            .unwrap_or("");
        if local == name && !tag_text.starts_with('/') && !tag_text.ends_with('/') {
            return Some(start + end + 1);
        }
        from = start;
    }
    None
}

fn find_close(rest: &str, name: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = rest[from..].find("</") {
        let start = from + i;
        let after = &rest[start + 2..];
        let end = after.find('>')?;
        let local = after[..end].trim().rsplit(':').next().unwrap_or("");
        if local == name {
            return Some(start);
        }
        from = start + 2;
    }
    None
}

fn xml_unescape_trim(s: &str) -> &str {
    s.trim()
}

/// Every `<name>…</name>` block.
fn blocks<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(open) = find_open(rest, name) {
        let body = &rest[open..];
        let Some(end) = find_close(body, name) else {
            break;
        };
        out.push(&body[..end]);
        rest = &body[end..];
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A SOAP fault from the router.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub code: u32,
    pub description: String,
}

impl Gateway {
    async fn soap(&self, action: &str, args: &[(&str, String)]) -> Result<String, Error> {
        let mut body = String::new();
        for (k, v) in args {
            let _ = write!(body, "<{k}>{}</{k}>", escape(v));
        }
        let envelope = format!(
            "<?xml version=\"1.0\"?>\r\n<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
             s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body>\
             <u:{action} xmlns:u=\"{}\">{body}</u:{action}></s:Body></s:Envelope>",
            self.service_type
        );
        let headers = [
            ("content-type", "text/xml; charset=\"utf-8\"".to_owned()),
            ("soapaction", format!("\"{}#{action}\"", self.service_type)),
        ];
        let (status, text) =
            http(http::Method::POST, &self.control_url, &headers, envelope).await?;
        if status.is_success() {
            return Ok(text);
        }
        let code = tag(&text, "errorCode")
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let description = tag(&text, "errorDescription").unwrap_or("").to_owned();
        Err(Error::Fault(Fault { code, description }))
    }

    /// The router's public address.
    pub async fn external_ip(&self) -> Result<IpAddr, Error> {
        let text = self.soap("GetExternalIPAddress", &[]).await?;
        tag(&text, "NewExternalIPAddress")
            .and_then(|ip| ip.parse().ok())
            .ok_or_else(|| Error::Router("the router did not give its public address".into()))
    }

    /// Forwards `port` on the router to `local:port` for `lease` (zero
    /// means until removed). Retries with a permanent lease on routers that
    /// only support those (error 725).
    pub async fn add(
        &self,
        protocol: Protocol,
        port: u16,
        local: IpAddr,
        lease: Duration,
        description: &str,
    ) -> Result<Duration, Error> {
        let args = |lease: Duration| {
            vec![
                ("NewRemoteHost", String::new()),
                ("NewExternalPort", port.to_string()),
                ("NewProtocol", protocol.as_str().to_owned()),
                ("NewInternalPort", port.to_string()),
                ("NewInternalClient", local.to_string()),
                ("NewEnabled", "1".to_owned()),
                ("NewPortMappingDescription", description.to_owned()),
                ("NewLeaseDuration", lease.as_secs().to_string()),
            ]
        };
        match self.soap("AddPortMapping", &args(lease)).await {
            Err(Error::Fault(Fault { code: 725, .. })) => {
                self.soap("AddPortMapping", &args(Duration::ZERO)).await?;
                Ok(Duration::ZERO)
            }
            Err(e) => Err(e),
            Ok(_) => Ok(lease),
        }
    }

    /// Removes a mapping. A mapping that is already gone (714) is fine.
    pub async fn remove(&self, protocol: Protocol, port: u16) -> Result<(), Error> {
        let args = [
            ("NewRemoteHost", String::new()),
            ("NewExternalPort", port.to_string()),
            ("NewProtocol", protocol.as_str().to_owned()),
        ];
        match self.soap("DeletePortMapping", &args).await {
            Ok(_) | Err(Error::Fault(Fault { code: 714, .. })) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tags_with_and_without_namespaces() {
        let xml = r#"<?xml version="1.0"?><root xmlns="urn:schemas-upnp-org:device-1-0">
          <device><friendlyName>Home Router</friendlyName><serviceList>
          <service><serviceType>urn:schemas-upnp-org:service:Layer3Forwarding:1</serviceType><controlURL>/l3f</controlURL></service>
          <service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
            <controlURL>/ctl/IPConn</controlURL></service>
          </serviceList></device></root>"#;
        assert_eq!(tag(xml, "friendlyName"), Some("Home Router"));
        let services = blocks(xml, "service");
        assert_eq!(services.len(), 2);
        assert_eq!(tag(services[1], "controlURL"), Some("/ctl/IPConn"));
        let soap = r#"<s:Envelope><s:Body><u:GetExternalIPAddressResponse xmlns:u="x">
            <NewExternalIPAddress>198.51.100.4</NewExternalIPAddress></u:GetExternalIPAddressResponse></s:Body></s:Envelope>"#;
        assert_eq!(tag(soap, "NewExternalIPAddress"), Some("198.51.100.4"));
        let fault = r#"<s:Fault><detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0">
            <errorCode>718</errorCode><errorDescription>ConflictInMappingEntry</errorDescription>
            </UPnPError></detail></s:Fault>"#;
        assert_eq!(tag(fault, "errorCode"), Some("718"));
    }

    #[test]
    fn reads_ssdp_headers() {
        let text = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nLocation: http://192.168.1.1:5000/rootDesc.xml\r\n\r\n";
        assert_eq!(
            header(text, "LOCATION"),
            Some("http://192.168.1.1:5000/rootDesc.xml")
        );
    }
}
