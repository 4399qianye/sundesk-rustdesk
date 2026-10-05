# RustDesk Virtual HID Driver

This directory contains the Windows VHF/KMDF source for the optional RustDesk
keyboard and relative mouse devices.

The driver is not built by Cargo. It requires the Windows Driver Kit (WDK) and
must be built and signed before it can be installed on a normal Windows host.
The user-mode RustDesk process sends reports through the `RustDeskHid` control
device.

The current first version exposes:

- a standard boot-protocol keyboard report;
- a standard relative mouse report with five buttons and vertical/horizontal wheel;
- one control IOCTL for submitting either report.

The production installer must ship a Microsoft-trusted signed package. The
package consists of `rustdesk_hid.inf`, `rustdesk_hid.cat`, and
`rustdesk_hid.sys`; the installer uses `pnputil /add-driver /install` so the
driver is registered through Windows Driver Store. A test-signed package is
suitable only for a test-signing Windows installation.

The CI expects a signing service with the `/sign/` endpoint used by
`bryangerlach/signing_api`. Configure `SIGN_BASE_URL` and `SIGN_API_KEY` in the
repository secrets. The signing service must use an EV certificate and the
driver must be submitted through Microsoft's Hardware Developer Program for
normal Windows 10/11 kernel-mode loading.

For machines with `usbip-win2` installed, RustDesk can use the bundled VIIPER
sidecar instead of this custom driver. Set `RUSTDESK_VIIPER=1` to enable that
backend. VIIPER creates standard HID mouse and keyboard devices through the
signed generic USB/IP driver; it is disabled by default.
