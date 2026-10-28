/*
MIT License

Copyright (c) 2026 Remote Screen contributors

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
*/

/*
The three signal/mode helpers below are adapted from Microsoft
Windows-driver-samples, video/IndirectDisplay/IddSampleDriver/Driver.cpp
(blob ae060298b725d4739c311ffe0f5711b224a76adb).
Copyright (c) Microsoft Corporation

The upstream LICENSE for those portions is retained here:

The Microsoft Public License (MS-PL)
Copyright (c) 2015 Microsoft

This license governs use of the accompanying software. If you use the software, you
accept this license. If you do not accept the license, do not use the software.

1. Definitions
The terms "reproduce," "reproduction," "derivative works," and "distribution" have the
same meaning here as under U.S. copyright law.
A "contribution" is the original software, or any additions or changes to the software.
A "contributor" is any person that distributes its contribution under this license.
"Licensed patents" are a contributor's patent claims that read directly on its contribution.

2. Grant of Rights
(A) Copyright Grant- Subject to the terms of this license, including the license conditions and limitations in section 3, each contributor grants you a non-exclusive, worldwide, royalty-free copyright license to reproduce its contribution, prepare derivative works of its contribution, and distribute its contribution or any derivative works that you create.
(B) Patent Grant- Subject to the terms of this license, including the license conditions and limitations in section 3, each contributor grants you a non-exclusive, worldwide, royalty-free license under its licensed patents to make, have made, use, sell, offer for sale, import, and/or otherwise dispose of its contribution in the software or derivative works of the contribution in the software.

3. Conditions and Limitations
(A) No Trademark License- This license does not grant you rights to use any contributors' name, logo, or trademarks.
(B) If you bring a patent claim against any contributor over patents that you claim are infringed by the software, your patent license from such contributor to the software ends automatically.
(C) If you distribute any portion of the software, you must retain all copyright, patent, trademark, and attribution notices that are present in the software.
(D) If you distribute any portion of the software in source code form, you may do so only under this license by including a complete copy of this license with your distribution. If you distribute any portion of the software in compiled or object code form, you may only do so under a license that complies with this license.
(E) The software is licensed "as-is." You bear the risk of using it. The contributors give no express warranties, guarantees or conditions. You may have additional consumer rights under your local laws which this license cannot change. To the extent permitted under your local laws, the contributors exclude the implied warranties of merchantability, fitness for a particular purpose and non-infringement.
*/

#include "RsIdd.h"
#include "MonitorRules.h"

#include <objbase.h>
#include <strsafe.h>

static void FillSignalInfo(DISPLAYCONFIG_VIDEO_SIGNAL_INFO& Mode, DWORD Width,
    DWORD Height, DWORD VSync, bool MonitorMode)
{
    Mode.totalSize.cx = Mode.activeSize.cx = Width;
    Mode.totalSize.cy = Mode.activeSize.cy = Height;
    Mode.AdditionalSignalInfo.vSyncFreqDivider = MonitorMode ? 0 : 1;
    Mode.AdditionalSignalInfo.videoStandard = 255;
    Mode.vSyncFreq.Numerator = VSync;
    Mode.vSyncFreq.Denominator = 1;
    Mode.hSyncFreq.Numerator = VSync * Height;
    Mode.hSyncFreq.Denominator = 1;
    Mode.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
    Mode.pixelRate = static_cast<UINT64>(VSync) * Width * Height;
}

static IDDCX_MONITOR_MODE CreateIddCxMonitorMode(DWORD Width, DWORD Height,
    DWORD VSync, IDDCX_MONITOR_MODE_ORIGIN Origin)
{
    IDDCX_MONITOR_MODE mode = {};
    mode.Size = sizeof(mode);
    mode.Origin = Origin;
    FillSignalInfo(mode.MonitorVideoSignalInfo, Width, Height, VSync, true);
    return mode;
}

static IDDCX_TARGET_MODE CreateIddCxTargetMode(DWORD Width, DWORD Height, DWORD VSync)
{
    IDDCX_TARGET_MODE mode = {};
    mode.Size = sizeof(mode);
    FillSignalInfo(mode.TargetVideoSignalInfo.targetVideoSignalInfo,
        Width, Height, VSync, false);
    return mode;
}

static NTSTATUS MonitorStatus(const wchar_t* Step, UINT32 MonitorId, NTSTATUS Status)
{
    WCHAR message[128] = {};
    if (SUCCEEDED(StringCchPrintfW(message, ARRAYSIZE(message),
        L"%ls monitor_id=%u NTSTATUS=0x%08lX\n", Step, MonitorId,
        static_cast<unsigned long>(Status)))) {
        OutputDebugStringW(message);
    }
    return Status;
}

static bool CopyRegisteredModes(RsIddDeviceContext* Ctx, UINT32 MonitorId,
    RSIDD_MODE* Modes, UINT32* ModeCount)
{
    if (Ctx == nullptr) {
        return false;
    }

    bool found = false;
    WdfWaitLockAcquire(Ctx->Lock, nullptr);
    for (const auto& entry : Ctx->Monitors) {
        if (entry.InUse && entry.MonitorId == MonitorId) {
            *ModeCount = entry.ModeCount;
            RtlCopyMemory(Modes, entry.Modes, entry.ModeCount * sizeof(RSIDD_MODE));
            found = true;
            break;
        }
    }
    WdfWaitLockRelease(Ctx->Lock);
    return found;
}

NTSTATUS RsIddAddMonitor(RsIddDeviceContext* Ctx, const RSIDD_ADD_IN* In, RSIDD_ADD_OUT* Out)
{
    if (rsidd::CheckAddIn(*In) != rsidd::AddCheck::Ok) {
        return MonitorStatus(L"AddValidate", In->monitor_id, STATUS_INVALID_PARAMETER);
    }

    NTSTATUS status = STATUS_SUCCESS;
    int slot = -1;
    IDDCX_ADAPTER adapter = nullptr;
    GUID containerId = {};

    WdfWaitLockAcquire(Ctx->Lock, nullptr);
    if (!Ctx->AdapterReady) {
        status = STATUS_DEVICE_NOT_READY;
    } else {
        bool inUse[RSIDD_MAX_MONITORS] = {};
        for (UINT32 i = 0; i < RSIDD_MAX_MONITORS; ++i) {
            const auto& entry = Ctx->Monitors[i];
            inUse[i] = entry.InUse != FALSE;
            if (entry.InUse && entry.MonitorId == In->monitor_id) {
                status = STATUS_OBJECT_NAME_COLLISION;
            }
        }
        if (NT_SUCCESS(status)) {
            slot = rsidd::FreeSlot(inUse, RSIDD_MAX_MONITORS);
            if (slot < 0) {
                status = STATUS_QUOTA_EXCEEDED;
            } else {
                auto& entry = Ctx->Monitors[slot];
                entry = {};
                entry.InUse = TRUE;
                entry.MonitorId = In->monitor_id;
                entry.ConnectorIndex = static_cast<UINT32>(slot);
                entry.Monitor = nullptr;
                entry.ModeCount = In->mode_count;
                RtlCopyMemory(entry.Modes, In->modes, In->mode_count * sizeof(RSIDD_MODE));
                RtlCopyMemory(entry.Edid, In->edid, RSIDD_EDID_LEN);
                if (FAILED(CoCreateGuid(&entry.ContainerId))) {
                    entry = {};
                    status = STATUS_UNSUCCESSFUL;
                } else {
                    containerId = entry.ContainerId;
                    adapter = Ctx->Adapter;
                }
            }
        }
    }
    WdfWaitLockRelease(Ctx->Lock);

    if (!NT_SUCCESS(status)) {
        return MonitorStatus(L"AddReserve", In->monitor_id, status);
    }

    BYTE edid[RSIDD_EDID_LEN] = {};
    RtlCopyMemory(edid, In->edid, sizeof(edid));
    IDDCX_MONITOR_INFO monitorInfo = {};
    monitorInfo.Size = sizeof(monitorInfo);
    monitorInfo.MonitorType = DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_WIRED;
    monitorInfo.ConnectorIndex = static_cast<UINT32>(slot);
    monitorInfo.MonitorDescription.Size = sizeof(monitorInfo.MonitorDescription);
    monitorInfo.MonitorDescription.Type = IDDCX_MONITOR_DESCRIPTION_TYPE_EDID;
    monitorInfo.MonitorDescription.DataSize = sizeof(edid);
    monitorInfo.MonitorDescription.pData = edid;
    monitorInfo.MonitorContainerId = containerId;

    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RsIddMonitorContext);
    IDARG_IN_MONITORCREATE createIn = {};
    createIn.ObjectAttributes = &attributes;
    createIn.pMonitorInfo = &monitorInfo;
    IDARG_OUT_MONITORCREATE createOut = {};
    status = IddCxMonitorCreate(adapter, &createIn, &createOut);
    if (!NT_SUCCESS(status)) {
        WdfWaitLockAcquire(Ctx->Lock, nullptr);
        Ctx->Monitors[slot] = {};
        WdfWaitLockRelease(Ctx->Lock);
        return MonitorStatus(L"MonitorCreate", In->monitor_id, status);
    }

    auto* monitorContext = RsIddGetMonitorContext(createOut.MonitorObject);
    monitorContext->Device = Ctx;
    monitorContext->MonitorId = In->monitor_id;
    monitorContext->Processor = nullptr;
    WdfWaitLockAcquire(Ctx->Lock, nullptr);
    Ctx->Monitors[slot].Monitor = createOut.MonitorObject;
    WdfWaitLockRelease(Ctx->Lock);

    // Arrival can synchronously reenter the EDID callback, which takes Lock.
    IDARG_OUT_MONITORARRIVAL arrival = {};
    status = IddCxMonitorArrival(createOut.MonitorObject, &arrival);
    if (!NT_SUCCESS(status)) {
        WdfObjectDelete(createOut.MonitorObject);
        WdfWaitLockAcquire(Ctx->Lock, nullptr);
        auto& entry = Ctx->Monitors[slot];
        if (entry.Monitor == createOut.MonitorObject) {
            entry = {};
        }
        WdfWaitLockRelease(Ctx->Lock);
        return MonitorStatus(L"MonitorArrival", In->monitor_id, status);
    }

    Out->monitor_id = In->monitor_id;
    Out->connector_index = static_cast<UINT32>(slot);
    return MonitorStatus(L"MonitorArrival", In->monitor_id, STATUS_SUCCESS);
}

NTSTATUS RsIddRemoveMonitor(RsIddDeviceContext* Ctx, UINT32 MonitorId)
{
    IDDCX_MONITOR monitor = nullptr;
    WdfWaitLockAcquire(Ctx->Lock, nullptr);
    for (auto& entry : Ctx->Monitors) {
        if (entry.InUse && entry.MonitorId == MonitorId && entry.Monitor != nullptr) {
            monitor = entry.Monitor;
            entry = {};
            break;
        }
    }
    WdfWaitLockRelease(Ctx->Lock);

    if (monitor == nullptr) {
        return MonitorStatus(L"MonitorRemove", MonitorId, STATUS_NOT_FOUND);
    }
    return MonitorStatus(L"MonitorDeparture", MonitorId, IddCxMonitorDeparture(monitor));
}

UINT32 RsIddRemoveAllMonitors(RsIddDeviceContext* Ctx)
{
    IDDCX_MONITOR monitors[RSIDD_MAX_MONITORS] = {};
    UINT32 monitorIds[RSIDD_MAX_MONITORS] = {};
    UINT32 count = 0;
    WdfWaitLockAcquire(Ctx->Lock, nullptr);
    for (auto& entry : Ctx->Monitors) {
        if (entry.Monitor != nullptr) {
            monitors[count] = entry.Monitor;
            monitorIds[count] = entry.MonitorId;
            ++count;
            entry = {};
        }
    }
    WdfWaitLockRelease(Ctx->Lock);

    for (UINT32 i = 0; i < count; ++i) {
        MonitorStatus(L"MonitorDeparture", monitorIds[i], IddCxMonitorDeparture(monitors[i]));
    }
    return count;
}

void RsIddFillInfo(RsIddDeviceContext* Ctx, RSIDD_INFO* Info)
{
    UINT32 active = 0;
    WdfWaitLockAcquire(Ctx->Lock, nullptr);
    for (const auto& entry : Ctx->Monitors) {
        if (entry.InUse && entry.Monitor != nullptr) {
            ++active;
        }
    }
    WdfWaitLockRelease(Ctx->Lock);
    rsidd::FillInfo(Info, active);
}

_Use_decl_annotations_
NTSTATUS RsIddParseMonitorDescription(const IDARG_IN_PARSEMONITORDESCRIPTION* pIn,
    IDARG_OUT_PARSEMONITORDESCRIPTION* pOut)
{
    if (pIn->MonitorDescription.Type != IDDCX_MONITOR_DESCRIPTION_TYPE_EDID ||
        pIn->MonitorDescription.DataSize < RSIDD_EDID_LEN ||
        pIn->MonitorDescription.pData == nullptr) {
        return MonitorStatus(L"ParseDescription", 0, STATUS_INVALID_PARAMETER);
    }

    const UINT32 serial = rsidd::EdidSerial(static_cast<const BYTE*>(pIn->MonitorDescription.pData));
    RSIDD_MODE modes[RSIDD_MAX_MODES] = {};
    UINT32 modeCount = 0;
    if (!CopyRegisteredModes(g_RsIddDevice, serial, modes, &modeCount)) {
        return MonitorStatus(L"ParseDescription", serial, STATUS_INVALID_PARAMETER);
    }

    pOut->MonitorModeBufferOutputCount = modeCount;
    pOut->PreferredMonitorModeIdx = 0;
    if (pIn->MonitorModeBufferInputCount < modeCount) {
        return MonitorStatus(L"ParseDescription", serial,
            pIn->MonitorModeBufferInputCount == 0 ? STATUS_SUCCESS : STATUS_BUFFER_TOO_SMALL);
    }
    if (pIn->pMonitorModes == nullptr) {
        return MonitorStatus(L"ParseDescription", serial, STATUS_INVALID_PARAMETER);
    }
    for (UINT32 i = 0; i < modeCount; ++i) {
        pIn->pMonitorModes[i] = CreateIddCxMonitorMode(modes[i].width, modes[i].height,
            modes[i].refresh_hz, IDDCX_MONITOR_MODE_ORIGIN_MONITORDESCRIPTOR);
    }
    return MonitorStatus(L"ParseDescription", serial, STATUS_SUCCESS);
}

_Use_decl_annotations_
NTSTATUS RsIddMonitorGetDefaultModes(IDDCX_MONITOR Monitor,
    const IDARG_IN_GETDEFAULTDESCRIPTIONMODES* pIn, IDARG_OUT_GETDEFAULTDESCRIPTIONMODES* pOut)
{
    UNREFERENCED_PARAMETER(Monitor);
    UNREFERENCED_PARAMETER(pIn);
    pOut->DefaultMonitorModeBufferOutputCount = 0;
    pOut->PreferredMonitorModeIdx = NO_PREFERRED_MODE;
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
NTSTATUS RsIddMonitorQueryModes(IDDCX_MONITOR Monitor,
    const IDARG_IN_QUERYTARGETMODES* pIn, IDARG_OUT_QUERYTARGETMODES* pOut)
{
    const auto* monitorContext = RsIddGetMonitorContext(Monitor);
    RSIDD_MODE modes[RSIDD_MAX_MODES] = {};
    UINT32 modeCount = 0;
    if (!CopyRegisteredModes(monitorContext->Device, monitorContext->MonitorId, modes, &modeCount)) {
        return MonitorStatus(L"QueryModes", monitorContext->MonitorId, STATUS_INVALID_PARAMETER);
    }

    pOut->TargetModeBufferOutputCount = modeCount;
    if (pIn->TargetModeBufferInputCount >= modeCount) {
        if (pIn->pTargetModes == nullptr) {
            return MonitorStatus(L"QueryModes", monitorContext->MonitorId, STATUS_INVALID_PARAMETER);
        }
        for (UINT32 i = 0; i < modeCount; ++i) {
            pIn->pTargetModes[i] = CreateIddCxTargetMode(modes[i].width, modes[i].height, modes[i].refresh_hz);
        }
    }
    return MonitorStatus(L"QueryModes", monitorContext->MonitorId, STATUS_SUCCESS);
}
