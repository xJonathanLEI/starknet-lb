use clap::{builder::TypedValueParser, error::ErrorKind, Arg, Command, Error};
use url::Url;

#[derive(Debug, Clone)]
pub enum UpstreamSpec {
    Raw(Url),
    Dns(DnsSpec),
}

#[derive(Debug, Clone)]
pub struct DnsSpec {
    pub host_port: String,
    pub path: String,
}

#[derive(Clone)]
pub struct UpstreamSpecParser;

impl TypedValueParser for UpstreamSpecParser {
    type Value = UpstreamSpec;

    fn parse_ref(
        &self,
        cmd: &Command,
        _arg: Option<&Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<Self::Value, Error> {
        match value.to_str() {
            Some(raw) => match raw.strip_prefix("dns:") {
                Some(dns) => {
                    let (host_port, path) = match dns.find('/') {
                        Some(ind_slash) => (&dns[0..ind_slash], &dns[ind_slash..]),
                        None => (dns, "/"),
                    };

                    let host_port = if host_port.contains(':') {
                        host_port.to_owned()
                    } else {
                        // Default Starknet RPC port: 9545
                        format!("{}:9545", host_port)
                    };

                    Ok(UpstreamSpec::Dns(DnsSpec {
                        host_port,
                        path: path.to_owned(),
                    }))
                }
                None => Ok(UpstreamSpec::Raw(Url::parse(raw).map_err(|_| {
                    cmd.clone().error(ErrorKind::InvalidValue, "invalid url")
                })?)),
            },
            None => Err(cmd
                .clone()
                .error(ErrorKind::InvalidValue, "invalid utf-8 spec")),
        }
    }
}
