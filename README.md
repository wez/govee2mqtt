> **⚠️ This is a patched fork of [wez/govee2mqtt](https://github.com/wez/govee2mqtt).**
> It fixes a UTF-8 crash that causes the bridge to crash-loop when Govee API returns Chinese preset names (affects H6076/H60B2 devices).
> See [Rollback Instructions](#rollback-to-upstream) below for when to switch back.

# Govee to MQTT bridge for Home Assistant

This repo provides a `govee` executable whose primary purpose is to act
as a bridge between [Govee](https://govee.com) devices and Home Assistant,
via the [Home Assistant MQTT Integration](https://www.home-assistant.io/integrations/mqtt/).

## What this fork changes

| Commit | File | Change |
|--------|------|--------|
| `0070e48` | `src/service/hass.rs` | Replace byte slicing (`camel[..1]`) with char iteration (`chars().next()`) to fix UTF-8 panic on non-ASCII preset names |
| `dcdf964` | `addon/config.yaml` | Bump version to `2026.03.14-0070e48-patched`, update name/image/url to distinguish from upstream |

**Upstream PR:** [wez/govee2mqtt#606](https://github.com/wez/govee2mqtt/pull/606) by theg1nger
**Upstream issue:** [wez/govee2mqtt#604](https://github.com/wez/govee2mqtt/issues/604)

## Features

* Robust LAN-first design. Not all of Govee's devices support LAN control,
  but for those that do, you'll have the lowest latency and ability to
  control them even when your primary internet connection is offline.
* Support for per-device modes and scenes.
* Support for the undocumented AWS IoT interface to your devices, provides
  low latency status updates.
* Support for the new [Platform
  API](https://developer.govee.com/reference/get-you-devices) in case the AWS
  IoT or LAN control is unavailable.

|Feature|Requires|Notes|
|-------|--------|-------------|
|DIY Scenes|API Key|Find in the list of Effects for the light in Home Assistant|
|Music Modes|API Key|Find in the list of Effects for the light in Home Assistant|
|Tap-to-Run / One Click Scene|IoT|Find in the overall list of Scenes in Home Assistant, as well as under the `Govee to MQTT` device|
|Live Device Status Updates|LAN and/or IoT|Devices typically report most changes within a couple of seconds.|
|Segment Color|API Key|Find the `Segment 00X` light entities associated with your main light device in Home Assistant|

* `API Key` means that you have [applied for a key from Govee](https://developer.govee.com/reference/apply-you-govee-api-key)
  and have configured it for use in govee2mqtt
* `IoT` means that you have configured your Govee account email and password for
  use in govee2mqtt, which will then attempt to use the
  *undocumented and likely unsupported* AWS MQTT-based IoT service
* `LAN` means that you have enabled the [Govee LAN API](https://app-h5.govee.com/user-manual/wlan-guide)
  on supported devices and that the LAN API protocol is functional on your network

## Usage

* [Installing the HASS Add-On](docs/ADDON.md) - for HAOS and Supervised HASS users
* [Running it in Docker](docs/DOCKER.md)
* [Configuration](docs/CONFIG.md)

## Have a question?

* [Is my device supported?](docs/SKUS.md)
* [Check out the FAQ](docs/FAQ.md)

## Rollback to upstream

Once [PR #606](https://github.com/wez/govee2mqtt/pull/606) is merged into `wez/govee2mqtt` and a new release is published, switch back to upstream:

1. **Check if the fix is merged:** Visit [wez/govee2mqtt#606](https://github.com/wez/govee2mqtt/pull/606) — if it says "Merged", you're good to go.
2. **In Home Assistant**, go to **Settings → Add-ons → Add-on Store** (three-dot menu → Repositories).
3. **Remove** this fork's repo URL: `https://github.com/homeassilol/govee2mqtt`
4. **Add** the upstream repo URL: `https://github.com/wez/govee2mqtt`
5. **Refresh** and update/reinstall the Govee2MQTT add-on.
6. **Restart** the add-on. Verify your Govee devices come back online.

If the upstream release version is newer than `2026.03.14-0070e48-patched`, you know you're on the official build.

## Want to show your support or gratitude?

It takes significant effort to build, maintain and support users of software
like this. If you can spare something to say thanks, it is appreciated!

* [Sponsor wez on Github](https://github.com/sponsors/wez)
* [Sponsor wez on Patreon](https://patreon.com/WezFurlong)
* [Sponsor wez on Ko-Fi](https://ko-fi.com/wezfurlong)
* [Sponsor wez via liberapay](https://liberapay.com/wez)

## Credits

This work is based on wez's earlier work with [Govee LAN
Control](https://github.com/wez/govee-lan-hass/).

The AWS IoT support was made possible by the work of @bwp91 in
[homebridge-govee](https://github.com/bwp91/homebridge-govee/).

The UTF-8 fix was originally authored by [theg1nger](https://github.com/wez/govee2mqtt/pull/606).
