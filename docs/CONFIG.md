# Configuration Options

## Govee Credentials

While `govee2mqtt` can run without any govee credentials, it can only discover
and control the devices for which you have already enabled LAN control.

It is recommended that you configure at least your Govee username and password
prior to your first run, as that is the only way for `govee2mqtt` to determine
room names to pre-assign your lights into the appropriate Home Assistant areas.

For scene control, for devices that don't support the LAN API, a Govee API Key
is required.  If you don't already have one, [you can find instructions on
obtaining one
here](https://developer.govee.com/reference/apply-you-govee-api-key).

|CLI|ENV|AddOn|Purpose|
|---|---|-----|-------|
|`--govee-email`|`GOVEE_EMAIL`|`govee_email`|The email address you registered with your govee account|
|`--govee-password`|`GOVEE_PASSWORD`|`govee_password`|The password you registered for your govee account|
|`--api-key`|`GOVEE_API_KEY`|`govee_api_key`|The API key you requested from Govee support|

*Concerned about sharing your credentials? See [Privacy](PRIVACY.md) for
information about how data is used and retained by `govee2mqtt`*

## LAN API Control

A number of Govee's devices support a local control protocol that doesn't require
your primary internet connection to be online.  This offers the lowest latency
for control and is the preferred way for `govee2mqtt` to interact with your
devices.

The [Govee LAN API is described in more detail
here](https://app-h5.govee.com/user-manual/wlan-guide), including a list of
supported devices.

*Note that you must use the Govee Home app to enable the LAN API for each
individual device before it will be possible for `govee2mqtt` to control
it via the LAN API.*

In theory the LAN API is zero-configuration and auto-discovery, but this
relies on your network supporting multicast-UDP, which is challenging
on some networks, especially across wifi access points and routers.

|CLI|ENV|AddOn|Purpose|
|---|---|-----|-------|
|`--no-multicast`|`GOVEE_LAN_NO_MULTICAST=true`|`no_multicast`|Do not multicast discovery packets to the Govee multicast group `239.255.255.250`. It is not recommended to use this option.|
|`--broadcast-all`|`GOVEE_LAN_BROADCAST_ALL=true`|`broadcast_all`|Enumerate all non-loopback network interfaces and send discovery packets to the broadcast address of each one, individually. This may be a good option if multicast-UDP doesn't work well on your network|
|`--global-broadcast`|`GOVEE_LAN_BROADCAST_GLOBAL=true`|`global_broadcast`|Send discovery packets to the global broadcast address `255.255.255.255`. This may be a possible solution if multicast-UDP doesn't work well on your network.|
|`--scan`|`GOVEE_LAN_SCAN=10.0.0.1,10.0.0.2`|`scan`|Specify a list of addresses that should be scanned by sending them discovery packets. Each element in the list can be an individual IP address (eg: the address of a specific device: be sure to assign it a static IP in your DHCP or other network setup!) or a network broadcast address like `10.0.0.255` for networks that are reachable but not directly plumbed on the machine where `govee2mqtt` is running.|

[Read more about LAN API Requirements here](LAN.md)

## MQTT Configuration

In order to make your devices appear in Home Assistant, you will need to have configured Home Assistant with an MQTT broker.

  * [follow these steps](https://www.home-assistant.io/integrations/mqtt/#configuration)

You will also need to configure `govee2mqtt` to use the same broker:

|CLI|ENV|AddOn|Purpose|
|---|---|-----|-------|
|`--mqtt-host`|`GOVEE_MQTT_HOST`|`mqtt_host`|The host name or IP address of your mqtt broker. This should be the same broker that you have configured in Home Assistant.|
|`--mqtt-port`|`GOVEE_MQTT_PORT`|`mqtt_port`|The port number of the mqtt broker. The default is `1883`|
|`--mqtt-username`|`GOVEE_MQTT_USER`|`mqtt_username`|If your broker requires authentication, the username to use|
|`--mqtt-password`|`GOVEE_MQTT_PASSWORD`|`mqtt_password`|If your broker requires authentication, the password to use|


## Software fade (`transition` support)

Home Assistant sends a `transition` (seconds) on `light.turn_on`. Govee's
cloud control API has no fade primitive, so when a light command includes
both `brightness` and a positive `transition`, `govee2mqtt` emulates the
fade bridge-side: it computes a step schedule up front and issues
intermediate brightness commands — the same ordinary control requests a
plain brightness set uses, so no Govee-side support is required and no
device capabilities change. There is no configuration for this feature.

Behavior notes:

  * Steps are spaced at least one second apart (Govee documents a
    per-device control budget of 2 requests/second, burst 6; one step per
    second leaves the rest for interleaved commands and polling). This
    floor applies to LAN-capable devices too, so fades pace identically
    regardless of transport. Durations clamp at 1 hour and fades at 1000
    commands.
  * Any newer command to the device — MQTT, HTTP API, scene, or another
    fade — supersedes an in-progress fade at the moment it takes the
    device control permit; no queued step of the superseded fade is sent
    afterwards (latest command wins). A retargeted fade continues from
    the brightness the interrupted fade had actually emitted. A fade
    that just completed keeps that continuity until its reconciling poll
    lands, so a re-applied command in that window (an HA brightness
    slider re-move defaults to `transition: 2`) ramps from where the
    light actually is, not from the pre-fade reported state.
  * A fade of an off light starts from zero: it fades up from dark rather
    than flashing on at whatever brightness preceded it being switched
    off.
  * Home Assistant learns the final value via the normal reconciling
    poll: a fade costs exactly one reconciling poll (scheduled for its
    final step — the same single poll a plain command costs, never a
    second one) and publishes no intermediate states. If a fade step
    fails, the remaining ramp is abandoned but the final target is
    attempted once more, so a device is not left stranded mid-ramp.
  * When many devices fade concurrently, a global floor of one fade step
    per 100 ms across the whole bridge keeps Govee's account-wide budget
    (12 requests/second, burst 80) intact; up to ~10 simultaneous fades
    are unaffected, beyond that step timestamps slip slightly while each
    fade still ends exactly at its target. A control command arriving
    during such a pause waits for it (bounded: ~100 ms per currently
    fading device) and then wins as always.

