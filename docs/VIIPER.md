# VIIPER Input Backend

RustDesk can optionally send Windows keyboard and relative mouse input through
VIIPER. VIIPER creates standard HID devices in user space and uses the signed
generic USB/IP driver from `usbip-win2`.

## Requirements

Install `usbip-win2` on the Windows target first. It provides the signed USB/IP
kernel driver and `usbip.exe`; VIIPER alone is not sufficient.

The Windows artifact contains `viiper.exe` and the `USBip-0.9.8.1-x64.exe`
installer beside `rustdesk.exe`. Install USB/IP once as Administrator, then
enable the backend with:

```powershell
$env:RUSTDESK_VIIPER = "1"
.\rustdesk.exe
```

Or set `RUSTDESK_VIIPER=1` permanently for the user or service that launches
RustDesk. Without this variable RustDesk keeps the normal input fallback.

VIIPER listens on localhost TCP port `3242`. RustDesk starts the sidecar when
needed, creates a virtual mouse and keyboard, and streams input reports to the
devices. The artifact includes `VIIPER-LICENSE.txt`. The VIIPER sidecar is
licensed GPL-3.0; the Rust client protocol is MIT. RustDesk communicates with
the standalone executable over TCP rather than linking the GPL core into the
RustDesk binary.

## Notes

- `usbip-win2` must be installed once on every target Windows machine.
- Run `USBip-0.9.8.1-x64.exe` as Administrator and reboot if the installer
  requests it.
- The first USB/IP installation may require a reboot and can briefly restart
  USB devices.
- VIIPER is currently opt-in through `RUSTDESK_VIIPER=1`.
- If VIIPER is unavailable, RustDesk falls back to the existing Windows input
  path.
