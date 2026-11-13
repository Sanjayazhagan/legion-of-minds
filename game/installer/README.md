# Windows installer

`build-installer.ps1` builds the release exe and packs it with the `assets` folder:

```
cd game\installer
.\build-installer.ps1 -Server https://play.example.com
```

It writes to `game\dist\`:

- `LegionOfMinds-<version>-windows.zip`: unzip anywhere and run `legion-of-minds.exe`.
- `LegionOfMinds-<version>-setup.exe`: an installer for the current user (no administrator prompt), with Start menu and optional desktop shortcuts and an uninstaller. Needs [Inno Setup 6](https://jrsoftware.org/isdl.php); without it you get the zip only.

`-Server` is the online server's address. It is built into the exe, so set it to the hosted server for any build you hand out. Players' saves live in `%APPDATA%\legion-of-minds` and are not touched by installing, updating or uninstalling.

The Release workflow (`.github/workflows/release.yml`) runs the same script on GitHub when a `v*` tag is pushed and attaches both files to the release. It reads the server address from the repository variable `BAG_SERVER` (Settings > Secrets and variables > Actions > Variables).

Release builds open no console window and carry the icon in `icon.ico` (`icon.png` is the same picture).
