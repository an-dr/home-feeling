# Install, update, and release

The same approach as [crossWall](https://github.com/an-dr/cross-wall).

## UC-INSTALL-01: Install the app

- **Actor**: Desktop user.
- **Goal**: Have the app in my Start Menu or application list, without administrator rights.
- **Flow**: Run the install script (`irm … | iex` on Windows, `curl … | sh` on Linux), or press Install in an unpacked build.
- **Success**: The app is installed per user under `~/.home-feeling`, is listed in the application list, and starts from there. The built-in Things are copied to the user's Things folder.

## UC-INSTALL-02: Remove the app

- **Actor**: Desktop user.
- **Goal**: Remove the app.
- **Flow**: Press Unregister in the Things window, then delete the install folder the app names.
- **Success**: The app leaves the application list. My Things folder is kept unless I delete it.

## UC-UPDATE-01: Update the app

- **Actor**: Desktop user.
- **Goal**: Move to the newest release.
- **Flow**: On the home-feeling settings page, Check for updates, press Update, restart.
- **Success**: The new version runs. A failed download leaves the running version as it was. The previous version stays installed. Things I edited are never overwritten; a newer built-in version of a Thing I changed is offered, not imposed.

## UC-RELEASE-01: Publish a release

- **Actor**: Maintainer.
- **Goal**: Ship a version.
- **Flow**: Set the version, push a matching tag.
- **Success**: CI tests and packages windows-x64, windows-arm64 and linux-x64, and publishes them as a GitHub release that installed apps update from.
