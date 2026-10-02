#include <Windows.h>
#include <cstdint>
#include <cstring>
#include <mutex>

namespace {
HANDLE hid_handle = INVALID_HANDLE_VALUE;
std::mutex hid_mutex;

HANDLE open_hid() {
    if (hid_handle != INVALID_HANDLE_VALUE) {
        return hid_handle;
    }
    hid_handle = CreateFileW(
        L"\\\\.\\RustDeskHid",
        GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        nullptr,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        nullptr);
    return hid_handle;
}

#pragma pack(push, 1)
struct HidReport {
    std::uint8_t device;
    std::uint8_t length;
    std::uint8_t data[8];
};
#pragma pack(pop)

constexpr DWORD kSubmitReport = CTL_CODE(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_WRITE_ACCESS);
}

extern "C" std::int32_t rustdesk_hid_submit(
    std::uint8_t device,
    const std::uint8_t *data,
    std::uint8_t length) {
    std::lock_guard<std::mutex> lock(hid_mutex);
    if (device == 0) {
        return open_hid() == INVALID_HANDLE_VALUE ? -1 : 0;
    }
    if (data == nullptr || length > sizeof(HidReport::data)) {
        return -1;
    }
    HANDLE handle = open_hid();
    if (handle == INVALID_HANDLE_VALUE) {
        return -1;
    }
    HidReport report{};
    report.device = device;
    report.length = length;
    memcpy(report.data, data, length);
    DWORD returned = 0;
    if (!DeviceIoControl(handle, kSubmitReport, &report, sizeof(report), nullptr, 0, &returned, nullptr)) {
        CloseHandle(handle);
        hid_handle = INVALID_HANDLE_VALUE;
        return -1;
    }
    return 0;
}
