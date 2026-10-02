#pragma once

#include <ntddk.h>
#include <wdmsec.h>
#include <vhf.h>

#define RUSTDESK_HID_POOL_TAG 'dHsR'
#define RUSTDESK_HID_DEVICE_NAME L"\\Device\\RustDeskHid"
#define RUSTDESK_HID_DOS_NAME L"\\DosDevices\\RustDeskHid"

#define IOCTL_RUSTDESK_HID_SUBMIT_REPORT \
    CTL_CODE(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_WRITE_ACCESS)

#define RUSTDESK_HID_REPORT_KEYBOARD 1
#define RUSTDESK_HID_REPORT_MOUSE 2

typedef struct _RUSTDESK_HID_REPORT {
    UCHAR Device;
    UCHAR Length;
    UCHAR Data[8];
} RUSTDESK_HID_REPORT, *PRUSTDESK_HID_REPORT;

typedef struct _RUSTDESK_HID_CONTEXT {
    VHFHANDLE VhfHandle;
    PDEVICE_OBJECT Device;
    KSPIN_LOCK ReportLock;
    UCHAR KeyboardReport[8];
    UCHAR MouseReport[7];
} RUSTDESK_HID_CONTEXT, *PRUSTDESK_HID_CONTEXT;

DRIVER_UNLOAD RustDeskHidUnload;
DRIVER_DISPATCH RustDeskHidCreateClose;
DRIVER_DISPATCH RustDeskHidDeviceControl;
