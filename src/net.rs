//! 局域网地址发现与 mDNS 广播。

use std::net::Ipv4Addr;

/// 枚举本机所有可用的局域网 IPv4 地址。
///
/// 不用"连一下 8.8.8.8 看内核选了哪个出口"那招：那招在没有默认网关的
/// 隔离局域网里会直接失败，而隔离局域网恰恰是这工具最典型的场景
/// （会议室、两台机器直连网线）。所以老老实实枚举网卡。
pub fn lan_ipv4_addresses() -> Vec<Ipv4Addr> {
    let mut addrs = Vec::new();
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return addrs;
    };

    for iface in ifaces {
        if iface.is_loopback() {
            continue;
        }
        let std::net::IpAddr::V4(v4) = iface.addr.ip() else {
            continue;
        };
        // 169.254.x.x 是没拿到 DHCP 时的自分配地址，对端连不上。
        if v4.is_link_local() {
            continue;
        }
        addrs.push(v4);
    }

    addrs.sort_by_key(|ip| (!ip.is_private(), *ip));
    addrs.dedup();
    addrs
}

/// 本机主机名（**不含** `.local` 后缀）。配合 Bonjour/mDNS，
/// 对端可以直接用 `主机名.local` 访问，不用记 IP——IP 会变，主机名不会。
pub fn hostname() -> String {
    let raw = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "wiretoss".to_string());

    // macOS 的 hostname 常常已经带上了 `.local`，调用方再拼一次就得到
    // `m4-max.local.local` —— 这个域名是解析不了的。
    raw.strip_suffix(".local")
        .map(str::to_string)
        .unwrap_or(raw)
}

/// 广播 `_wiretoss._tcp.local.` 服务。
///
/// 返回的 daemon 必须一直被持有：它一旦被 drop，服务就从网络上消失了。
/// 所以调用方要把它存到生命周期够长的地方。
pub fn advertise(port: u16) -> Option<mdns_sd::ServiceDaemon> {
    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            // 广播失败不是致命问题——手输 IP 照样能用，不该因此拒绝启动。
            tracing::warn!("mDNS 服务启动失败（不影响使用，只是无法用主机名访问）：{e}");
            return None;
        }
    };

    let host = hostname();
    let host_fqdn = format!("{host}.local.");
    let instance = format!("Wiretoss@{host}");

    // 多网卡时只广播一个地址会让对端连不上，所以先用一个占位地址构造，
    // 再 `enable_addr_auto()` 让 daemon 在注册时把所有本地地址都带上。
    let seed = lan_ipv4_addresses()
        .first()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());

    let info = match mdns_sd::ServiceInfo::new(
        "_wiretoss._tcp.local.",
        &instance,
        &host_fqdn,
        seed.as_str(),
        port,
        &[("version", env!("CARGO_PKG_VERSION"))][..],
    ) {
        Ok(i) => i.enable_addr_auto(),
        Err(e) => {
            tracing::warn!("mDNS 服务信息构造失败：{e}");
            return None;
        }
    };

    if let Err(e) = daemon.register(info) {
        tracing::warn!("mDNS 注册失败：{e}");
        return None;
    }
    Some(daemon)
}

/// 把 URL 渲染成终端二维码。手机扫一下就能打开，省去手敲 IP。
pub fn qr_to_terminal(url: &str) -> Option<String> {
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    // Dense1x2 用半个字符块表示一个模块，宽高比才对得上；
    // 用普通字符渲染出来的二维码在终端里是拉长的，很多手机扫不出来。
    Some(
        code.render::<qrcode::render::unicode::Dense1x2>()
            .quiet_zone(true)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_is_never_empty_and_has_no_local_suffix() {
        let h = hostname();
        assert!(!h.is_empty());
        assert!(
            !h.ends_with(".local"),
            "hostname 不该带 .local 后缀（拼出来会变成 .local.local）：{h}"
        );
    }

    #[test]
    fn qr_renders_for_a_url() {
        let qr = qr_to_terminal("http://192.168.1.5:8080/").unwrap();
        assert!(qr.lines().count() > 5);
    }

    #[test]
    fn lan_addresses_exclude_link_local_and_loopback() {
        for ip in lan_ipv4_addresses() {
            assert!(!ip.is_loopback());
            assert!(!ip.is_link_local());
        }
    }
}
