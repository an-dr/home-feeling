# Things

Seeing, changing, reloading, and trusting Things. A Thing is a Lua script with its manifest, in its own folder inside the user's Things folder.

## UC-THINGS-01: See and control my Things

- **Actor**: Desktop user.
- **Goal**: Know which Things are installed and turn each on or off.
- **Flow**: Open the Things window, from the tray icon on Windows. It lists every Thing with its state (running, stopped, faulted, waiting for permission), and each Thing has an enable switch.
- **Success**: A disabled Thing stops at once and stays stopped after a restart. An enabled one starts.

## UC-THINGS-06: Keep it in the tray

- **Actor**: Desktop user.
- **Goal**: Have the app running without a window in the way.
- **Flow**: Close the Things window: the app keeps running in the tray, and so do its Things. Double-click the tray icon, choose Show Things, or launch the app again to bring the window back; Quit in the tray menu ends it. In the Things window, Start with Windows adds the app to the user's startup list, and Start in the tray makes that start open no window.
- **Success**: Closing the window never stops the Things. A second launch never opens a second copy; it shows the running one, except when it is the startup entry starting an app that already runs. Quit stops everything.

## UC-THINGS-02: Change a Thing and reload it

- **Actor**: Thing author.
- **Goal**: See an edit to a Thing's code or config take effect without restarting the app.
- **Flow**: Edit `init.lua` or `manifest.toml` in the Thing's folder, save, and press Reload all in the Things window or the tray menu.
- **Success**: The Thing runs the new code. If the new version fails to load, the previous one keeps running and the Things window shows the error with file and line.

## UC-THINGS-03: Find out why a Thing failed

- **Actor**: Thing author.
- **Goal**: Fix a broken Thing.
- **Flow**: Open the Thing in the Things window.
- **Success**: Its log and its last error are shown: a Lua error with file and line, a refused permission naming the topic, or a quarantine naming the exceeded budget. A faulted Thing never stops other Things or the app.

## UC-THINGS-04: Approve what a Thing may do

- **Actor**: Desktop user.
- **Goal**: Run a Thing only with the powers I agreed to.
- **Flow**: A new Thing, or one whose manifest asks for more than before, waits in the Things window until the user reviews the requested capabilities (for example "move and resize windows", "global hotkeys") and allows or refuses them.
- **Success**: The Thing runs only with approved capabilities. Anything it was not granted is refused and logged. The decision is remembered until the request changes.

## UC-THINGS-05: Adjust a Thing's settings

- **Actor**: Desktop user.
- **Goal**: Change a Thing's options.
- **Flow**: Select the Thing in the Things window and open its Settings tab. The settings are grouped in sub-tabs by the section each declares in its manifest, one card per setting: a switch, a slider, a choice, a hotkey recorder (click it, then press the keys; Esc cancels, Delete unbinds; a combination another setting uses is refused), or a colour picker (a swatch, a hex field, preset colours, and hue, saturation, and brightness sliders). Reset all settings to defaults clears every choice.
- **Success**: The Thing reloads with the new value a moment after the change and the choice is remembered across restarts. An invalid value in a text field is explained under it and is not applied, so the Thing keeps its previous setting.
