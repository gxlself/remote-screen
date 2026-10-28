/*++

Copyright (c) Microsoft Corporation

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

Abstract:

    This module contains a sample implementation of an indirect display driver. See the included README.md file and the
    various TODO blocks throughout this file and all accompanying files for information on building a production driver.

    MSDN documentation on indirect displays can be found at https://msdn.microsoft.com/en-us/library/windows/hardware/mt761968(v=vs.85).aspx.

Environment:

    User Mode, UMDF

--*/

// MIT License (adapted from Microsoft Windows-driver-samples).
// Modified for Remote Screen

#include <windows.h>
#include <initguid.h>
#include "RsIdd.h"

#include <cstdarg>
#include <cstdio>

#include <wdf.h>

#include "SwapChain.h"

RsIddDeviceContext* g_RsIddDevice = nullptr;

namespace
{
    void TraceLog(_In_z_ PCWSTR Format, ...)
    {
        WCHAR buffer[256] = {};
        va_list arguments;
        va_start(arguments, Format);
        const int result = _vsnwprintf_s(buffer, ARRAYSIZE(buffer), _TRUNCATE, Format, arguments);
        va_end(arguments);

        if (result < 0)
        {
            buffer[ARRAYSIZE(buffer) - 1] = L'\0';
        }

        OutputDebugStringW(buffer);
        OutputDebugStringW(L"\n");
    }

    void TraceStatus(_In_z_ PCWSTR Step, NTSTATUS Status)
    {
        TraceLog(L"RsIdd %ls status=0x%08lX", Step, static_cast<ULONG>(Status));
    }
}

extern "C" DRIVER_INITIALIZE DriverEntry;

static EVT_WDF_DRIVER_DEVICE_ADD RsIddDeviceAdd;
static EVT_WDF_DEVICE_D0_ENTRY RsIddD0Entry;
static EVT_WDF_DEVICE_D0_EXIT RsIddD0Exit;

static EVT_IDD_CX_ADAPTER_INIT_FINISHED RsIddAdapterInitFinished;
static EVT_IDD_CX_ADAPTER_COMMIT_MODES RsIddAdapterCommitModes;

extern "C" BOOL WINAPI DllMain(
    HINSTANCE Instance,
    UINT Reason,
    LPVOID Reserved)
{
    UNREFERENCED_PARAMETER(Instance);
    UNREFERENCED_PARAMETER(Reason);
    UNREFERENCED_PARAMETER(Reserved);
    return TRUE;
}

_Use_decl_annotations_
extern "C" NTSTATUS DriverEntry(PDRIVER_OBJECT DriverObject, PUNICODE_STRING RegistryPath)
{
    WDF_DRIVER_CONFIG config;
    WDF_DRIVER_CONFIG_INIT(&config, RsIddDeviceAdd);

    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);

    const NTSTATUS status = WdfDriverCreate(
        DriverObject,
        RegistryPath,
        &attributes,
        &config,
        WDF_NO_HANDLE);
    if (!NT_SUCCESS(status))
    {
        TraceStatus(L"DriverEntry/WdfDriverCreate", status);
    }

    return status;
}

_Use_decl_annotations_
static NTSTATUS RsIddDeviceAdd(WDFDRIVER Driver, PWDFDEVICE_INIT DeviceInit)
{
    UNREFERENCED_PARAMETER(Driver);

    WDF_PNPPOWER_EVENT_CALLBACKS powerCallbacks;
    WDF_PNPPOWER_EVENT_CALLBACKS_INIT(&powerCallbacks);
    powerCallbacks.EvtDeviceD0Entry = RsIddD0Entry;
    powerCallbacks.EvtDeviceD0Exit = RsIddD0Exit;
    WdfDeviceInitSetPnpPowerEventCallbacks(DeviceInit, &powerCallbacks);

    IDD_CX_CLIENT_CONFIG clientConfig;
    IDD_CX_CLIENT_CONFIG_INIT(&clientConfig);
    clientConfig.EvtIddCxAdapterInitFinished = RsIddAdapterInitFinished;
    clientConfig.EvtIddCxAdapterCommitModes = RsIddAdapterCommitModes;
    clientConfig.EvtIddCxParseMonitorDescription = RsIddParseMonitorDescription;
    clientConfig.EvtIddCxMonitorGetDefaultDescriptionModes = RsIddMonitorGetDefaultModes;
    clientConfig.EvtIddCxMonitorQueryTargetModes = RsIddMonitorQueryModes;
    clientConfig.EvtIddCxMonitorAssignSwapChain = RsIddAssignSwapChain;
    clientConfig.EvtIddCxMonitorUnassignSwapChain = RsIddUnassignSwapChain;
    clientConfig.EvtIddCxDeviceIoControl = RsIddEvtDeviceIoControl;

    NTSTATUS status = IddCxDeviceInitConfig(DeviceInit, &clientConfig);
    if (!NT_SUCCESS(status))
    {
        TraceStatus(L"DeviceAdd/IddCxDeviceInitConfig", status);
        return status;
    }

    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RsIddDeviceContext);

    WDFDEVICE device = nullptr;
    status = WdfDeviceCreate(&DeviceInit, &attributes, &device);
    if (!NT_SUCCESS(status))
    {
        TraceStatus(L"DeviceAdd/WdfDeviceCreate", status);
        return status;
    }

    status = IddCxDeviceInitialize(device);
    if (!NT_SUCCESS(status))
    {
        TraceStatus(L"DeviceAdd/IddCxDeviceInitialize", status);
        return status;
    }

    RsIddDeviceContext* context = RsIddGetContext(device);
    context->Device = device;
    context->Adapter = nullptr;
    context->AdapterReady = FALSE;
    context->Lock = nullptr;
    ZeroMemory(context->Monitors, sizeof(context->Monitors));
    context->LastControlTick = static_cast<LONGLONG>(GetTickCount64());
    context->LeaseTimer = nullptr;

    status = WdfWaitLockCreate(WDF_NO_OBJECT_ATTRIBUTES, &context->Lock);
    if (!NT_SUCCESS(status))
    {
        TraceStatus(L"DeviceAdd/WdfWaitLockCreate", status);
        return status;
    }

    status = WdfDeviceCreateDeviceInterface(device, &GUID_DEVINTERFACE_RSIDD, nullptr);
    if (!NT_SUCCESS(status))
    {
        TraceStatus(L"DeviceAdd/WdfDeviceCreateDeviceInterface", status);
        return status;
    }

    g_RsIddDevice = context;
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
static NTSTATUS RsIddD0Entry(WDFDEVICE Device, WDF_POWER_DEVICE_STATE PreviousState)
{
    UNREFERENCED_PARAMETER(PreviousState);

    RsIddDeviceContext* context = RsIddGetContext(Device);
    if (context == nullptr)
    {
        TraceStatus(L"D0Entry/no-context", STATUS_INVALID_DEVICE_STATE);
        return STATUS_INVALID_DEVICE_STATE;
    }

    context->AdapterReady = FALSE;

    IDDCX_ADAPTER_CAPS adapterCaps = {};
    adapterCaps.Size = sizeof(adapterCaps);
    adapterCaps.MaxMonitorsSupported = RSIDD_MAX_MONITORS;
    adapterCaps.EndPointDiagnostics.Size = sizeof(adapterCaps.EndPointDiagnostics);
    adapterCaps.EndPointDiagnostics.TransmissionType = IDDCX_TRANSMISSION_TYPE_WIRED_OTHER;
    adapterCaps.EndPointDiagnostics.GammaSupport = IDDCX_FEATURE_IMPLEMENTATION_NONE;
    adapterCaps.EndPointDiagnostics.pEndPointFriendlyName = L"Remote Screen Virtual Display";
    adapterCaps.EndPointDiagnostics.pEndPointManufacturerName = L"Remote Screen";
    adapterCaps.EndPointDiagnostics.pEndPointModelName = L"RsIdd";

    IDDCX_ENDPOINT_VERSION endpointVersion = {};
    endpointVersion.Size = sizeof(endpointVersion);
    endpointVersion.MajorVer = 1;
    endpointVersion.MinorVer = 0;
    endpointVersion.Build = 0;
    endpointVersion.SKU = 0;
    adapterCaps.EndPointDiagnostics.pFirmwareVersion = &endpointVersion;
    adapterCaps.EndPointDiagnostics.pHardwareVersion = &endpointVersion;

    WDF_OBJECT_ATTRIBUTES adapterAttributes;
    WDF_OBJECT_ATTRIBUTES_INIT(&adapterAttributes);

    IDARG_IN_ADAPTER_INIT adapterInit = {};
    adapterInit.WdfDevice = Device;
    adapterInit.pCaps = &adapterCaps;
    adapterInit.ObjectAttributes = &adapterAttributes;

    IDARG_OUT_ADAPTER_INIT adapterInitOut = {};
    const NTSTATUS status = IddCxAdapterInitAsync(&adapterInit, &adapterInitOut);
    if (!NT_SUCCESS(status))
    {
        TraceStatus(L"D0Entry/IddCxAdapterInitAsync", status);
        return status;
    }

    context->Adapter = adapterInitOut.AdapterObject;
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
static NTSTATUS RsIddD0Exit(WDFDEVICE Device, WDF_POWER_DEVICE_STATE TargetState)
{
    UNREFERENCED_PARAMETER(TargetState);

    RsIddDeviceContext* context = RsIddGetContext(Device);
    if (context == nullptr)
    {
        TraceStatus(L"D0Exit/no-context", STATUS_INVALID_DEVICE_STATE);
        return STATUS_INVALID_DEVICE_STATE;
    }

    RsIddControlStop(context);
    context->AdapterReady = FALSE;
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
static NTSTATUS RsIddAdapterInitFinished(
    IDDCX_ADAPTER Adapter,
    const IDARG_IN_ADAPTER_INIT_FINISHED* InArgs)
{
    UNREFERENCED_PARAMETER(Adapter);

    RsIddDeviceContext* context = g_RsIddDevice;
    if (context == nullptr)
    {
        TraceStatus(L"AdapterInitFinished/no-device", STATUS_DEVICE_NOT_READY);
        return STATUS_DEVICE_NOT_READY;
    }

    if (!NT_SUCCESS(InArgs->AdapterInitStatus))
    {
        TraceStatus(L"AdapterInitFinished/adapter", InArgs->AdapterInitStatus);
        return STATUS_SUCCESS;
    }

    WdfWaitLockAcquire(context->Lock, nullptr);
    context->AdapterReady = TRUE;
    WdfWaitLockRelease(context->Lock);

    const NTSTATUS status = RsIddControlStart(context);
    if (!NT_SUCCESS(status))
    {
        TraceStatus(L"AdapterInitFinished/control-start", status);
    }

    return status;
}

_Use_decl_annotations_
static NTSTATUS RsIddAdapterCommitModes(
    IDDCX_ADAPTER Adapter,
    const IDARG_IN_COMMITMODES* InArgs)
{
    UNREFERENCED_PARAMETER(Adapter);
    UNREFERENCED_PARAMETER(InArgs);
    return STATUS_SUCCESS;
}
