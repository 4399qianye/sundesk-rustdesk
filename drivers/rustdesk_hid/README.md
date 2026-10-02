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

The production installer must ship a Microsoft-trusted signed package. A
test-signed package is suitable only for a test-signing Windows installation.
