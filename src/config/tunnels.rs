use super::{AppError, ConfigManager, Connection, Result};
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr};

/// Both identities are ordinary SSH profiles. Only the endpoint on the VPS is
/// stored here: the device profile owns the local port and the device Host Key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelRoute {
    pub gateway_id: String,
    pub device_id: String,
    pub service_port: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::manager::AuthMethod;

    #[test]
    fn saving_routes_uses_existing_profiles_and_preserves_both_identities() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("devices.toml");
        let mut cfg = ConfigManager::with_path(&path).unwrap();
        let mut vps = Connection::new(
            "gateway.example".into(),
            22,
            "gateway".into(),
            AuthMethod::None,
        );
        vps.id = "gateway".into();
        let mut dev = Connection::new("127.0.0.1".into(), 29022, "dev".into(), AuthMethod::None);
        dev.id = "dev".into();
        cfg.add_connection(vps.clone()).unwrap();
        cfg.add_connection(dev.clone()).unwrap();
        let route = TunnelRoute {
            gateway_id: vps.id.clone(),
            device_id: dev.id.clone(),
            service_port: 24022,
        };
        cfg.save_tunnel(None, route.clone()).unwrap();
        let mut read = ConfigManager::with_path(&path).unwrap();
        assert_eq!(read.tunnels(), &[route.clone()]);
        assert_eq!(
            read.tunnel_endpoints(&route).unwrap().1.to_string(),
            "127.0.0.1:29022"
        );
        assert!(read.save_tunnel(None, route.clone()).is_err());
        let mut switched = dev.clone();
        switched.host = "elsewhere.example".into();
        assert!(read.update_connection(switched).is_err());
        assert!(read.remove_connection(&vps.id).is_err());
        read.remove_tunnel(0).unwrap();
        assert!(read.tunnels().is_empty());
        assert_eq!(read.connections().len(), 2);
        let reverse = TunnelRoute {
            gateway_id: dev.id,
            device_id: vps.id,
            service_port: 24022,
        };
        assert!(read.save_tunnel(None, reverse).is_err());
        assert!(
            ConfigManager::with_path(&path)
                .unwrap()
                .tunnels()
                .is_empty()
        );
    }
}

impl ConfigManager {
    pub fn tunnels(&self) -> &[TunnelRoute] {
        &self.config.tunnels
    }

    pub(crate) fn tunnel_endpoints(&self, r: &TunnelRoute) -> Result<(Connection, SocketAddr)> {
        let invalid = |s: &str| AppError::ValidationError(s.into());
        let mut gateway = self
            .find_connection(&r.gateway_id)
            .cloned()
            .ok_or_else(|| invalid("Choose a saved VPS SSH profile."))?;
        let device = self
            .find_connection(&r.device_id)
            .ok_or_else(|| invalid("Choose a saved device SSH profile."))?;
        if gateway.id == device.id || gateway.same_endpoint(device) {
            return Err(invalid(
                "The VPS and device must have separate SSH endpoints.",
            ));
        }
        if device.host != "127.0.0.1" || device.port == 0 {
            return Err(invalid(
                "Set the device profile host to 127.0.0.1 and give it a local port.",
            ));
        }
        if r.service_port == 0 {
            return Err(invalid("VPS service port must be between 1 and 65535."));
        }
        self.apply_host_pin(&mut gateway)?;
        Ok((
            gateway,
            SocketAddr::from((Ipv4Addr::LOCALHOST, device.port)),
        ))
    }

    pub(crate) fn save_tunnel(
        &mut self,
        prior: Option<usize>,
        route: TunnelRoute,
    ) -> Result<usize> {
        let (_, addr) = self.tunnel_endpoints(&route)?;
        if self.tunnels().iter().enumerate().any(|(i, r)| {
            Some(i) != prior
                && (r.device_id == route.device_id
                    || self
                        .find_connection(&r.device_id)
                        .is_some_and(|d| d.port == addr.port()))
        }) {
            return Err(AppError::ValidationError(
                "Each tunnel needs its own device and local port.".into(),
            ));
        }
        let mut candidate = self.config.clone();
        let index = prior.unwrap_or(candidate.tunnels.len());
        if let Some(i) = prior {
            candidate.tunnels[i] = route;
        } else {
            candidate.tunnels.push(route);
        }
        self.persist(&candidate)?;
        self.config = candidate;
        Ok(index)
    }

    pub(crate) fn remove_tunnel(&mut self, index: usize) -> Result<()> {
        let mut candidate = self.config.clone();
        candidate.tunnels.remove(index);
        self.persist(&candidate)?;
        self.config = candidate;
        Ok(())
    }
}
