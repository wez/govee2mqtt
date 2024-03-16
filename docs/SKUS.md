# Which SKUs work with Govee2MQTT?

Support depends largely on what Govee exposes via its documented APIs.
There are some devices for which the undocumented APIs have been
reverse engineered.

If the device has no WiFi, then Govee2MQTT is not able to control
it at this time, as there is no BLE support in Govee2MQTT at this time.

Only devices that support the LAN API are able to be controlled fully
locally. All known LAN API compatible devices are lights; there are
no known applicance devices that support fully local control. This
is not a limitation of Govee2MQTT, but a limitation of the hardware
itself.

|Family|LAN API?|Platform API?|Undocumented API?|
|------|--------|-------------|-----------------|
|Lights/LED Strips|The more modern/powerful WiFi controller chips can have LAN API enabled through the Govee App. When enabled, the device can have its color/temperature, brightness and on/off state controlled locally, with no external network connection required.|Most WiFi enabled controller chips can be controlled via Govee's cloud-based Platform API, and this is necessary to control features like light effect modes and scenes.|Most WiFi enabled controller chips can trigger state changes notifications via IoT for fast state updates in the HA UI|
|Humidifiers|Not supported by these devices|Most humidifiers are controllable via the Platform API, but the level of control can be patchy; some models cannot have their night lights controlled fully at this time due to bugs on Govee's side.|Only the H7160 at this time. It allows control over the night light|
|Kettles|Not supported by these devices|Tested with H7171 and H7173|No|
|Heaters, Fans, Purifiers|Not supported by these devices|Tested with H7101, H7102, H7111, H7121, H7130, H7131, H713A, H7135|No|
|Plugs|Not supported by these devices|Yes, but the API is buggy and support may be limited. ([H5082](https://github.com/wez/govee2mqtt/issues/65))|No|

