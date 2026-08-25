TODO

 # Device polling

At the moment if we disconnect a board and attach a new one and press flash it always fails the first time, this seems to be because the app state is incorrect (also it says MCU connected even after disconnecting). I suspect that this means we should be intermittently polling the device over SWD to ensure that we're still connected or if a new connection is there and be ready appropriately to flash either way

 # Provisioning
* One of the main purpose of the PortalTestBench app is to provision new boards
    * Keep a persistent running number for the ID (essentially the count of previously provisioned boards + 1)
    * This number is user editable
    * The next board that gets flashed gets this serial number (stored to flash somewhere. We need to assign some 'partition space' for flash usage e.g. at the end of the flash range)
    * Serial numbers can be polled over RS485 and are shown in the serial startup
    * Put it in the info at the top (reduce some of the other info)
    * Before flashing, check if this board already has an ID (maybe ID's need a significant checksum to ensure that they are real ID's and not badly read data) and use that one if it pre-exists (note in interface if so)
Provisioning is essentially 'flashing'

After flashing ensure the firmware runs (for virgin devices this might need an unplug and replug)
For unplug+replug, detect this is happening and don't flash in that case if we're in auto-flash mode (e.g. check the ID and the firmware version check out correctly)

Nove flash/auto-flash to the top of operations

Keep a database of provisions (eventually this might go online but keep it local for now), keeping track of the MCU data, the time, the version, the serial number (also show the history of actions against this serial number in the interface)

We want also to have a current level per module. If we can't verify a reliable home happened on an axis, then we should boost to 100% current (if not already), and if that succeeds, to persist that setting. Also be able to set this option in the flasher settings. This means again having a section of flash for settings that can be read/written to from the flasher app and the application both.

# Layout

Do a pass on the layout to reduce space usage whilst keeping clear functionality, e.g.:
* Flash manually / autoamtic flashing can go into the same div as READY (and can remove 'disarmed')
* Reduce height of MCU connected section (eg. smaller / more compact info about firmware, ST-Link, etc)
* Fixture banks could be 2 columns (bootloader, application)
* We don't need 'SELECTED' as well as the blue tick both in the selection panels
* 'full selected' can go into the area instead of 'Click a selected bank to leave it out'
* If there's an error it currently apears in the MCU connected panel, in the Ready panel and in the status log at the bottom. Just the status log should be fine since it's highlighted there

# Home Flag survey (planned)

* Ability to survey the home flag area for optical sensors (like we did previously in python with a movement range, step distance and value range - just make the GUI much better in the app than it was before. nice graph, nice interface for starting the routine. survey is done in a range relative to the known home position, suitable defaults)
    * This requires quite different controls over serial. I can imagine a mode where you enter 'direct mode' which isn't the normal menu but a more intensive serial comms mode for debugging/inspecting things. It doesn't need to be human readable, can be pure binary, and we can replace some of our other existing usage patterns with this mode.

# Clean out PortalFlasher (planned)
TestBench seems to have taken over all functionality from flasher - remove the old flasher app from the repo

# Progress improvements
* Monotonic progress whilst flashing (don't keep going back to 0 between steps)
* Better progress dring procedures (e.g. startup - parse the incoming messages better and show a plausible % based on previous measurements of progress)