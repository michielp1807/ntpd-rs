# Setting up an NTP server

By default, ntpd-rs only acts as an NTP client, and doesn't serve time on any
network interface. To enable ntpd-rs as a server, the following can be added to
the configuration:
```toml
[[server]]
listen = "0.0.0.0:123"
```
This will cause ntpd-rs to listen on all network interfaces on UDP port 123 for
NTP client requests. If you only want to listen on a specific network
interface, change `0.0.0.0` to the IP address of that interface.

You can now configure a different machine to use your new server by adding to
its configuration:
```toml
[[source]]
mode = "server"
address = "<your server ip>:123"
```

## Limiting access
If you only want specific IP addresses to be able to access the server, you can
configure a list of allowed clients through the allowlist mechanism. For this,
edit the server configuration to look like:
```toml
[[server]]
listen = "0.0.0.0:123"
[server.allowlist]
filter = ["<allowed ipv4 1>/32", "<allowed ipv4 2>/32", "<allowed ipv6 1>/128"]
action = "ignore"
```
When configured this way, your server will only respond to the listed IP
addresses. The IP addresses are written in CIDR notation, which means you can
allow entire subnets at a time by specifying the size of the subnet instead of
the 32 or 128 after the slash. For example, `192.168.1.1/24` will allow any IP
address of the form `192.168.1.*`.

If you want to block certain IP addresses from accessing the server, you can
configure a list of blocked clients as follows:
```toml
[[server]]
listen = "0.0.0.0:123"
[server.denylist]
filter = ["<blocked ipv4 1>/32", "<blocked ipv4 2>/32", "<blocked ipv6 1>/128"]
action = "deny"
```
The deny list uses the same CIDR notion as the allow list, and can also be used
to block subnets. Connections from IP addresses contained in the deny list will
always be blocked, even if they also happen to be in the allow list.

The allow and deny list configurations are both optional in ntpd-rs. By
default, if a server is configured it will accept traffic from anywhere. When
configuring both allow and deny lists, ntpd-rs will first check if a remote is
on the deny list. Only if this is not the case will the allow list be
considered.

The `allowlist.action` and `denylist.action` properties can have two values:

- `ignore` silently ignores the request
- `deny` sends a deny kiss-o'-death packet

## Adding your server to the NTP pool

If your NTP server has a public IP address, you can consider making it
available as part of the [NTP pool](https://www.ntppool.org). Please note that
this can have significant long-term impact in terms of NTP traffic to that
particular IP address. Please read [the join instructions](https://www.ntppool.org/en/join.html)
carefully before joining the pool.

