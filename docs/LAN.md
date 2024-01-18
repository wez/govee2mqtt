# Govee LAN API Information

[Govee's LAN control API](https://app-h5.govee.com/user-manual/wlan-guide) is a
UDP based protocol with the following requirements:

* Govee2MQTT must be able to bind to UDP port `4002` on the machine where it runs
* Each Govee device must individually have had its LAN API access enabled
  in its settings in the Govee Home App
* UDP ports 4001 and 4003 must be reachable on each Govee device

## Device Discovery

Govee devices with LAN protocol enabled will listen for discovery packets
UDP port 4001.  They join the multicast group `239.255.255.250` so that
a client performing discovery, in theory, only needs to multicast to
that same group and limit the amount of network traffic expended on
discovery.

In practice, multicast-UDP is not well supported by various routers, especially
on WiFI enabled networks.

Govee2MQTT provides a couple of options that can help in situations where
multicast-UDP isn't working well in your environment, or where you have more
unusual network topology.

* You can specify a list of IP address to which discovery packets should
  be sent directly
* You can specify a number of variations on regular UDP broadcasts that
  might work better than multicast in some situations

For a device to be shown as usable via the LAN API in Govee2MQTT:

* UDP ports `4001` and `4003` must both be reachable from the Govee2MQTT instance
* The Govee device will respond to the source IP address of the packets sent
  from Govee2MQTT, but UDP port `4002` will be used instead of the originating
  port. Your network must allow this sort of "reply" to route back to Govee2MQTT.

See [LAN API Control Config](CONFIG.md#lan-api-control) for more details on how
to configure these options.

## Router / Network Setup tips

* Some routers have optimizations that prevent multicast-UDP from crossing from
  the WLAN to the LAN. Check your router's manual and configuration options.
  Don't confuse it with multicast-DNS. While that also uses UDP, it is a
  specialization and having that working doesn't imply that multicast-UDP in
  general will work.

* Consider enabling the `broadcast_all` option for the addon, which uses
  explicit UDP broadcasts to each network interface, rather than multicast.

* Assign a static IP to the device in your DHCP setup, then add that IP to the
  [scan list](CONFIG.md#lan-api-control) in the addon config, which will use
  unicast UDP packets to each device.  This is heavier on your network, but
  more compatible with certain VLAN setups.

* If you have an IOT VLAN or similar, ensure that your firewall is not blocking
  the ports mentioned above

