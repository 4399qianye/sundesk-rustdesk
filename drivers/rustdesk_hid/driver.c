#include "rustdesk_hid.h"

static RUSTDESK_HID_CONTEXT g_Context;

static const GUID RustDeskHidClassGuid =
    {0xA4D5B0E6, 0x7D4B, 0x4DC8, {0x9D, 0xB5, 0x7A, 0xE3, 0xC6, 0xA0, 0xF4, 0x11}};

static const UCHAR ReportDescriptor[] = {
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x01,
    0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00,
    0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02,
    0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x06,
    0x75, 0x08, 0x15, 0x00, 0x25, 0x65, 0x19, 0x00,
    0x29, 0x65, 0x81, 0x00, 0xC0,
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02,
    0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01,
    0x29, 0x05, 0x15, 0x00, 0x25, 0x01, 0x95, 0x05,
    0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x03,
    0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31,
    0x16, 0x00, 0x80, 0x26, 0xFF, 0x7F, 0x75, 0x10,
    0x95, 0x02, 0x81, 0x06, 0x09, 0x38, 0x15, 0x81,
    0x25, 0x7F, 0x75, 0x08, 0x95, 0x02, 0x81, 0x06,
    0xC0, 0xC0
};

static NTSTATUS Complete(PIRP irp, NTSTATUS status, ULONG_PTR information)
{
    irp->IoStatus.Status = status;
    irp->IoStatus.Information = information;
    IoCompleteRequest(irp, IO_NO_INCREMENT);
    return status;
}

static VOID SubmitReport(UCHAR device, const UCHAR *data, ULONG length)
{
    HID_XFER_PACKET packet;
    RtlZeroMemory(&packet, sizeof(packet));
    packet.reportBuffer = (PUCHAR)data;
    packet.reportBufferLen = length;
    packet.reportId = device;
    (VOID)VhfReadReportSubmit(g_Context.VhfHandle, &packet);
}

NTSTATUS RustDeskHidCreateClose(PDEVICE_OBJECT device, PIRP irp)
{
    UNREFERENCED_PARAMETER(device);
    return Complete(irp, STATUS_SUCCESS, 0);
}

NTSTATUS RustDeskHidDeviceControl(PDEVICE_OBJECT device, PIRP irp)
{
    UNREFERENCED_PARAMETER(device);
    PIO_STACK_LOCATION stack = IoGetCurrentIrpStackLocation(irp);
    if (stack->Parameters.DeviceIoControl.IoControlCode != IOCTL_RUSTDESK_HID_SUBMIT_REPORT ||
        stack->Parameters.DeviceIoControl.InputBufferLength < sizeof(RUSTDESK_HID_REPORT)) {
        return Complete(irp, STATUS_INVALID_DEVICE_REQUEST, 0);
    }

    PRUSTDESK_HID_REPORT input = (PRUSTDESK_HID_REPORT)irp->AssociatedIrp.SystemBuffer;
    if ((input->Device != RUSTDESK_HID_REPORT_KEYBOARD &&
         input->Device != RUSTDESK_HID_REPORT_MOUSE) ||
        input->Length > sizeof(input->Data)) {
        return Complete(irp, STATUS_INVALID_PARAMETER, 0);
    }

    UCHAR report[8];
    KIRQL oldIrql;
    KeAcquireSpinLock(&g_Context.ReportLock, &oldIrql);
    if (input->Device == RUSTDESK_HID_REPORT_KEYBOARD) {
        RtlCopyMemory(g_Context.KeyboardReport, input->Data, input->Length);
        RtlCopyMemory(report, g_Context.KeyboardReport, input->Length);
    } else {
        RtlCopyMemory(g_Context.MouseReport, input->Data, input->Length);
        RtlCopyMemory(report, g_Context.MouseReport, input->Length);
    }
    KeReleaseSpinLock(&g_Context.ReportLock, oldIrql);
    SubmitReport(input->Device, report, input->Length);
    return Complete(irp, STATUS_SUCCESS, 0);
}

VOID RustDeskHidUnload(PDRIVER_OBJECT driver)
{
    UNICODE_STRING dosName;
    RtlInitUnicodeString(&dosName, RUSTDESK_HID_DOS_NAME);
    IoDeleteSymbolicLink(&dosName);
    if (g_Context.VhfHandle != NULL) {
        VhfDelete(g_Context.VhfHandle, TRUE);
        g_Context.VhfHandle = NULL;
    }
    if (g_Context.Device != NULL) {
        IoDeleteDevice(g_Context.Device);
        g_Context.Device = NULL;
    }
}

NTSTATUS DriverEntry(PDRIVER_OBJECT driver, PUNICODE_STRING registryPath)
{
    UNREFERENCED_PARAMETER(registryPath);
    RtlZeroMemory(&g_Context, sizeof(g_Context));
    KeInitializeSpinLock(&g_Context.ReportLock);
    driver->DriverUnload = RustDeskHidUnload;
    driver->MajorFunction[IRP_MJ_CREATE] = RustDeskHidCreateClose;
    driver->MajorFunction[IRP_MJ_CLOSE] = RustDeskHidCreateClose;
    driver->MajorFunction[IRP_MJ_DEVICE_CONTROL] = RustDeskHidDeviceControl;

    UNICODE_STRING deviceName;
    UNICODE_STRING dosName;
    RtlInitUnicodeString(&deviceName, RUSTDESK_HID_DEVICE_NAME);
    RtlInitUnicodeString(&dosName, RUSTDESK_HID_DOS_NAME);
    NTSTATUS status = IoCreateDeviceSecure(
        driver,
        sizeof(RUSTDESK_HID_CONTEXT),
        &deviceName,
        FILE_DEVICE_UNKNOWN,
        FILE_DEVICE_SECURE_OPEN,
        FALSE,
        &SDDL_DEVOBJ_SYS_ALL_ADM_RWX_WORLD_RW_RES_R,
        &RustDeskHidClassGuid,
        &g_Context.Device);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    status = IoCreateSymbolicLink(&dosName, &deviceName);
    if (!NT_SUCCESS(status)) {
        RustDeskHidUnload(driver);
        return status;
    }

    VHF_CONFIG config;
    VHF_CONFIG_INIT(&config,
        g_Context.Device,
        sizeof(ReportDescriptor),
        (PUCHAR)ReportDescriptor);
    config.VendorID = 0x28DE;
    config.ProductID = 0x11A0;
    config.VersionNumber = 1;
    status = VhfCreate(&config, &g_Context.VhfHandle);
    if (NT_SUCCESS(status)) {
        status = VhfStart(g_Context.VhfHandle);
    }
    if (!NT_SUCCESS(status)) {
        RustDeskHidUnload(driver);
    }
    return status;
}
