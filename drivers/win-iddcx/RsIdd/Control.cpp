// Remote Screen virtual display driver: control channel.
// Portions are adapted from Microsoft's Windows-driver-samples
// video/IndirectDisplay/IddSampleDriver (MIT License).
#include "RsIdd.h"

#include <cstdio>

namespace
{
constexpr ULONGLONG RSIDD_LEASE_MS = 5000;
constexpr ULONG RSIDD_LEASE_TICK_MS = 1000;

_Use_decl_annotations_
VOID LeaseTick(WDFTIMER Timer);

PCWSTR IoctlName(ULONG IoControlCode)
{
    switch (IoControlCode)
    {
    case IOCTL_RSIDD_GET_INFO:
        return L"GET_INFO";
    case IOCTL_RSIDD_ADD:
        return L"ADD";
    case IOCTL_RSIDD_REMOVE:
        return L"REMOVE";
    case IOCTL_RSIDD_REMOVE_ALL:
        return L"REMOVE_ALL";
    case IOCTL_RSIDD_PING:
        return L"PING";
    default:
        return L"UNKNOWN";
    }
}

void CompleteRequest(WDFREQUEST Request, ULONG IoControlCode, NTSTATUS Status,
                     ULONG_PTR Information)
{
    WCHAR message[160] = {};
    (void)swprintf_s(message, ARRAYSIZE(message),
                     L"RsIdd IOCTL %ls status=0x%08lX\n", IoctlName(IoControlCode),
                     static_cast<unsigned long>(Status));
    OutputDebugStringW(message);
    WdfRequestCompleteWithInformation(Request, Status, Information);
}

void CompleteError(WDFREQUEST Request, ULONG IoControlCode, NTSTATUS Status)
{
    CompleteRequest(Request, IoControlCode, Status, 0);
}

_Use_decl_annotations_
VOID LeaseTick(WDFTIMER Timer)
{
    if (Timer == nullptr)
    {
        return;
    }

    const WDFDEVICE device = static_cast<WDFDEVICE>(WdfTimerGetParentObject(Timer));
    if (device == nullptr)
    {
        return;
    }

    RsIddDeviceContext* context = RsIddGetContext(device);
    if (context == nullptr)
    {
        return;
    }

    const LONGLONG lastControlTick =
        InterlockedCompareExchange64(&context->LastControlTick, 0, 0);
    const ULONGLONG now = GetTickCount64();
    if (lastControlTick < 0 || now <= static_cast<ULONGLONG>(lastControlTick) ||
        now - static_cast<ULONGLONG>(lastControlTick) <= RSIDD_LEASE_MS)
    {
        return;
    }

    RSIDD_INFO info = {};
    RsIddFillInfo(context, &info);
    if (info.active_monitors == 0)
    {
        return;
    }

    const UINT32 removed = RsIddRemoveAllMonitors(context);
    WCHAR message[96] = {};
    (void)swprintf_s(message, ARRAYSIZE(message), L"RsIdd lease expired, removed %u\n", removed);
    OutputDebugStringW(message);
}
} // namespace

_Use_decl_annotations_
void RsIddEvtDeviceIoControl(WDFDEVICE Device, WDFREQUEST Request, size_t OutputBufferLength,
                             size_t InputBufferLength, ULONG IoControlCode)
{
    UNREFERENCED_PARAMETER(OutputBufferLength);
    UNREFERENCED_PARAMETER(InputBufferLength);

    RsIddDeviceContext* context = nullptr;
    if (Device != nullptr)
    {
        context = RsIddGetContext(Device);
        if (context != nullptr)
        {
            // A request renews the lease before any validation, including a bad request.
            InterlockedExchange64(&context->LastControlTick,
                                  static_cast<LONGLONG>(GetTickCount64()));
        }
    }

    if (context == nullptr)
    {
        CompleteError(Request, IoControlCode, STATUS_INVALID_DEVICE_STATE);
        return;
    }

    switch (IoControlCode)
    {
    case IOCTL_RSIDD_GET_INFO:
    {
        RSIDD_VERSION_IN* input = nullptr;
        size_t inputLength = 0;
        NTSTATUS status = WdfRequestRetrieveInputBuffer(
            Request, sizeof(RSIDD_VERSION_IN), reinterpret_cast<PVOID*>(&input), &inputLength);
        if (!NT_SUCCESS(status))
        {
            CompleteError(Request, IoControlCode, status);
            return;
        }

        RSIDD_INFO* output = nullptr;
        size_t outputLength = 0;
        status = WdfRequestRetrieveOutputBuffer(
            Request, sizeof(RSIDD_INFO), reinterpret_cast<PVOID*>(&output), &outputLength);
        if (!NT_SUCCESS(status))
        {
            CompleteError(Request, IoControlCode, status);
            return;
        }

        if (input->version != RSIDD_VERSION)
        {
            CompleteError(Request, IoControlCode, STATUS_REVISION_MISMATCH);
            return;
        }

        RsIddFillInfo(context, output);
        CompleteRequest(Request, IoControlCode, STATUS_SUCCESS, sizeof(RSIDD_INFO));
        return;
    }

    case IOCTL_RSIDD_ADD:
    {
        RSIDD_ADD_IN* input = nullptr;
        size_t inputLength = 0;
        NTSTATUS status = WdfRequestRetrieveInputBuffer(
            Request, sizeof(RSIDD_ADD_IN), reinterpret_cast<PVOID*>(&input), &inputLength);
        if (!NT_SUCCESS(status))
        {
            CompleteError(Request, IoControlCode, status);
            return;
        }

        RSIDD_ADD_OUT* output = nullptr;
        size_t outputLength = 0;
        status = WdfRequestRetrieveOutputBuffer(
            Request, sizeof(RSIDD_ADD_OUT), reinterpret_cast<PVOID*>(&output), &outputLength);
        if (!NT_SUCCESS(status))
        {
            CompleteError(Request, IoControlCode, status);
            return;
        }

        if (input->version != RSIDD_VERSION)
        {
            CompleteError(Request, IoControlCode, STATUS_REVISION_MISMATCH);
            return;
        }

        status = RsIddAddMonitor(context, input, output);
        CompleteRequest(Request, IoControlCode, status,
                        NT_SUCCESS(status) ? sizeof(RSIDD_ADD_OUT) : 0);
        return;
    }

    case IOCTL_RSIDD_REMOVE:
    {
        RSIDD_REMOVE_IN* input = nullptr;
        size_t inputLength = 0;
        NTSTATUS status = WdfRequestRetrieveInputBuffer(
            Request, sizeof(RSIDD_REMOVE_IN), reinterpret_cast<PVOID*>(&input), &inputLength);
        if (!NT_SUCCESS(status))
        {
            CompleteError(Request, IoControlCode, status);
            return;
        }

        if (input->version != RSIDD_VERSION)
        {
            CompleteError(Request, IoControlCode, STATUS_REVISION_MISMATCH);
            return;
        }

        status = RsIddRemoveMonitor(context, input->monitor_id);
        CompleteError(Request, IoControlCode, status);
        return;
    }

    case IOCTL_RSIDD_REMOVE_ALL:
    {
        RSIDD_VERSION_IN* input = nullptr;
        size_t inputLength = 0;
        NTSTATUS status = WdfRequestRetrieveInputBuffer(
            Request, sizeof(RSIDD_VERSION_IN), reinterpret_cast<PVOID*>(&input), &inputLength);
        if (!NT_SUCCESS(status))
        {
            CompleteError(Request, IoControlCode, status);
            return;
        }

        RSIDD_REMOVE_ALL_OUT* output = nullptr;
        size_t outputLength = 0;
        status = WdfRequestRetrieveOutputBuffer(Request, sizeof(RSIDD_REMOVE_ALL_OUT),
                                                reinterpret_cast<PVOID*>(&output), &outputLength);
        if (!NT_SUCCESS(status))
        {
            CompleteError(Request, IoControlCode, status);
            return;
        }

        if (input->version != RSIDD_VERSION)
        {
            CompleteError(Request, IoControlCode, STATUS_REVISION_MISMATCH);
            return;
        }

        output->removed = RsIddRemoveAllMonitors(context);
        CompleteRequest(Request, IoControlCode, STATUS_SUCCESS, sizeof(RSIDD_REMOVE_ALL_OUT));
        return;
    }

    case IOCTL_RSIDD_PING:
    {
        RSIDD_VERSION_IN* input = nullptr;
        size_t inputLength = 0;
        NTSTATUS status = WdfRequestRetrieveInputBuffer(
            Request, sizeof(RSIDD_VERSION_IN), reinterpret_cast<PVOID*>(&input), &inputLength);
        if (!NT_SUCCESS(status))
        {
            CompleteError(Request, IoControlCode, status);
            return;
        }

        if (input->version != RSIDD_VERSION)
        {
            CompleteError(Request, IoControlCode, STATUS_REVISION_MISMATCH);
            return;
        }

        CompleteRequest(Request, IoControlCode, STATUS_SUCCESS, 0);
        return;
    }

    default:
        CompleteError(Request, IoControlCode, STATUS_INVALID_DEVICE_REQUEST);
        return;
    }
}

NTSTATUS RsIddControlStart(RsIddDeviceContext* Ctx)
{
    if (Ctx == nullptr || Ctx->Device == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }
    if (Ctx->LeaseTimer != nullptr)
    {
        return STATUS_SUCCESS;
    }

    WDF_TIMER_CONFIG timerConfig;
    WDF_TIMER_CONFIG_INIT_PERIODIC(&timerConfig, LeaseTick, RSIDD_LEASE_TICK_MS);

    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    attributes.ExecutionLevel = WdfExecutionLevelPassive;
    attributes.ParentObject = Ctx->Device;

    WDFTIMER timer = nullptr;
    NTSTATUS status = WdfTimerCreate(&timerConfig, &attributes, &timer);
    if (!NT_SUCCESS(status))
    {
        return status;
    }

    Ctx->LeaseTimer = timer;
    InterlockedExchange64(&Ctx->LastControlTick, static_cast<LONGLONG>(GetTickCount64()));
    if (!WdfTimerStart(timer, WDF_REL_TIMEOUT_IN_MS(RSIDD_LEASE_TICK_MS)))
    {
        Ctx->LeaseTimer = nullptr;
        return STATUS_UNSUCCESSFUL;
    }
    return STATUS_SUCCESS;
}

void RsIddControlStop(RsIddDeviceContext* Ctx)
{
    if (Ctx != nullptr && Ctx->LeaseTimer != nullptr)
    {
        WdfTimerStop(Ctx->LeaseTimer, TRUE);
        // The timer remains parented to the device and is deleted with it. Clear the
        // handle so a later D0Entry can create a running timer again.
        Ctx->LeaseTimer = nullptr;
    }
}
