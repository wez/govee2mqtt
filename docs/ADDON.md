# Installing as a Home Assistant Add-On

If you are running HAOS or Supervised Home Assistant, then your
installation is compatible with Home Assistant Add-Ons.

If you installed Home Assistant through a different technique,
you cannot install Add-Ons and will need to use a different
approach to setting up Govee2MQTT.

## Overview

The process is relatively easy, it's just a lot of clicks
in different places because of the way the UI is laid out.

The broad steps are:

* Enable advanced mode to allow installing Govee2MQTT
  from this repository.
* Install a MQTT Broker from the Add-On store
* Enable the MQTT Integration
* Add this repository to your Add-On store
* Install Govee2MQTT
* Configure it
* Start it

## Installation

### Enable Advanced Mode

Go to your user profile; click on your profile icon in the
bottom left of the screen.  Scroll down and turn on "Advanced Mode"
so that you will be able to see Govee2MQTT in the list of Add-Ons
when we get to that point.

![image](https://github.com/wez/govee-lan-hass/assets/117777/444c399d-0a91-41bf-804e-efcbabe17635)

### Set up MQTT

1. Go to the Add-Ons section of the settings: https://my.home-assistant.io/redirect/supervisor
2. Click on the "Adds-On Store" button in the bottom right corner
3. Look for the "Mosquitto Broker"
    * Click on it
    * Install it
    * Start it
4. Go to "Settings", then "Devices & Services" and you should see a tile offering to enable the MQTT integration. Click on it and enable it.

### Now Install Govee2MQTT

1. Go to the Add-Ons section of the settings: https://my.home-assistant.io/redirect/supervisor
2. Click on the "Adds-On Store" button in the bottom right corner
3. Look for the 3 vertically stacked dots in the top right corner:

![image](https://github.com/wez/govee-lan-hass/assets/117777/c425615b-d7be-4ff2-a0d9-c8b7cfb8b63e)

4. Click on "Repositories"
5. Enter `https://github.com/wez/govee2mqtt` and click "Add"
6. You should see:

![image](https://github.com/wez/govee-lan-hass/assets/117777/a2603e2d-dec1-4711-8d94-c957bf4a7a01)

7. Click "close"
8. You should now see:

![image](https://github.com/wez/govee-lan-hass/assets/117777/4e70f5e4-d54e-4e95-94db-b1d4a562eab1)

9. Click on it
10. Click "install"
11. At the top of the screen is a "Configuration" tab, click it

![image](https://github.com/wez/govee-lan-hass/assets/117777/fd2953b5-a576-4ab4-a903-0330a749ae97)

12. Check the "Show unused optional configuration options" option
13. Fill out at least the first three options; the govee email, password and api key

14. Click "Save" (bottom right)
15. Click on the "Info" tab (top of screen)
16. Now you can click "Start" to launch it

### Verify

1. You can use the "Logs" tab (top right) to see diagnostics
2. After a couple of seconds, your devices should be discovered and show up under the MQTT integration

