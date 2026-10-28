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

    Swap-chain processing support for an indirect display driver.

Environment:

    User Mode, UMDF

--*/

// MIT License (adapted from Microsoft Windows-driver-samples).
// Modified for Remote Screen

#include "SwapChain.h"

#include <new>
#include <utility>

#include <windows.h>

using Microsoft::WRL::ComPtr;

RsIddD3DDevice::RsIddD3DDevice(LUID adapterLuid)
    : m_adapterLuid(adapterLuid)
{
}

HRESULT RsIddD3DDevice::Initialize()
{
    HRESULT result = CreateDXGIFactory2(0, IID_PPV_ARGS(&m_dxgiFactory));
    if (FAILED(result))
    {
        return result;
    }

    result = m_dxgiFactory->EnumAdapterByLuid(m_adapterLuid, IID_PPV_ARGS(&m_adapter));
    if (FAILED(result))
    {
        return result;
    }

    result = D3D11CreateDevice(
        m_adapter.Get(),
        D3D_DRIVER_TYPE_UNKNOWN,
        nullptr,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        nullptr,
        0,
        D3D11_SDK_VERSION,
        &Device,
        nullptr,
        &m_deviceContext);
    return result;
}

RsIddSwapChainProcessor::RsIddSwapChainProcessor(
    IDDCX_SWAPCHAIN swapChain,
    std::shared_ptr<RsIddD3DDevice> device,
    HANDLE nextSurfaceAvailable)
    : m_swapChain(swapChain),
      m_device(std::move(device)),
      m_nextSurfaceAvailable(nextSurfaceAvailable),
      m_terminateEvent(CreateEventW(nullptr, TRUE, FALSE, nullptr)),
      m_thread(nullptr)
{
    if (m_terminateEvent != nullptr)
    {
        m_thread = CreateThread(nullptr, 0, RunThread, this, 0, nullptr);
    }
}

RsIddSwapChainProcessor::~RsIddSwapChainProcessor()
{
    if (m_terminateEvent != nullptr)
    {
        SetEvent(m_terminateEvent);
    }

    if (m_thread != nullptr)
    {
        WaitForSingleObject(m_thread, INFINITE);
        CloseHandle(m_thread);
        m_thread = nullptr;
    }

    if (m_terminateEvent != nullptr)
    {
        CloseHandle(m_terminateEvent);
        m_terminateEvent = nullptr;
    }

    if (m_swapChain != nullptr)
    {
        WdfObjectDelete(reinterpret_cast<WDFOBJECT>(m_swapChain));
        m_swapChain = nullptr;
    }
}

DWORD WINAPI RsIddSwapChainProcessor::RunThread(LPVOID argument)
{
    auto* processor = static_cast<RsIddSwapChainProcessor*>(argument);
    processor->Run();
    return 0;
}

void RsIddSwapChainProcessor::Run()
{
    DWORD taskIndex = 0;
    HANDLE avrtHandle = AvSetMmThreadCharacteristicsW(L"Distribution", &taskIndex);

    RunCore();

    if (m_swapChain != nullptr)
    {
        WdfObjectDelete(reinterpret_cast<WDFOBJECT>(m_swapChain));
        m_swapChain = nullptr;
    }

    if (avrtHandle != nullptr)
    {
        AvRevertMmThreadCharacteristics(avrtHandle);
    }
}

void RsIddSwapChainProcessor::RunCore()
{
    if (m_device == nullptr || m_swapChain == nullptr || m_terminateEvent == nullptr)
    {
        return;
    }

    ComPtr<IDXGIDevice> dxgiDevice;
    HRESULT result = m_device->Device.As(&dxgiDevice);
    if (FAILED(result))
    {
        return;
    }

    IDARG_IN_SWAPCHAINSETDEVICE setDevice = {};
    setDevice.pDevice = dxgiDevice.Get();
    result = IddCxSwapChainSetDevice(m_swapChain, &setDevice);
    if (FAILED(result))
    {
        return;
    }

    for (;;)
    {
        IDARG_OUT_RELEASEANDACQUIREBUFFER buffer = {};
        result = IddCxSwapChainReleaseAndAcquireBuffer(m_swapChain, &buffer);

        if (result == E_PENDING)
        {
            HANDLE waitHandles[] = {m_nextSurfaceAvailable, m_terminateEvent};
            const DWORD waitResult = WaitForMultipleObjects(ARRAYSIZE(waitHandles), waitHandles, FALSE, 16);
            if (waitResult == WAIT_OBJECT_0 || waitResult == WAIT_TIMEOUT)
            {
                continue;
            }

            break;
        }

        if (FAILED(result))
        {
            break;
        }

        // The surface is intentionally not read or copied. Releasing this
        // reference immediately lets IddCx recycle the producer buffer.
        if (buffer.MetaData.pSurface != nullptr)
        {
            ComPtr<IDXGIResource> acquiredSurface;
            acquiredSurface.Attach(buffer.MetaData.pSurface);
            acquiredSurface.Reset();
        }

        result = IddCxSwapChainFinishedProcessingFrame(m_swapChain);
        if (FAILED(result))
        {
            break;
        }
    }
}

_Use_decl_annotations_
NTSTATUS RsIddAssignSwapChain(
    IDDCX_MONITOR Monitor,
    const IDARG_IN_SETSWAPCHAIN* InArgs)
{
    RsIddMonitorContext* context = RsIddGetMonitorContext(Monitor);
    if (context == nullptr || context->Device == nullptr || InArgs == nullptr)
    {
        if (InArgs != nullptr && InArgs->hSwapChain != nullptr)
        {
            WdfObjectDelete(reinterpret_cast<WDFOBJECT>(InArgs->hSwapChain));
        }
        return STATUS_INVALID_PARAMETER;
    }

    delete context->Processor;
    context->Processor = nullptr;

    std::shared_ptr<RsIddD3DDevice> device(new (std::nothrow) RsIddD3DDevice(InArgs->RenderAdapterLuid));
    if (device == nullptr || FAILED(device->Initialize()))
    {
        WdfObjectDelete(reinterpret_cast<WDFOBJECT>(InArgs->hSwapChain));
        return STATUS_GRAPHICS_INDIRECT_DISPLAY_ABANDON_SWAPCHAIN;
    }

    auto* processor = new (std::nothrow) RsIddSwapChainProcessor(
        InArgs->hSwapChain,
        std::move(device),
        InArgs->hNextSurfaceAvailable);
    if (processor == nullptr)
    {
        WdfObjectDelete(reinterpret_cast<WDFOBJECT>(InArgs->hSwapChain));
        return STATUS_GRAPHICS_INDIRECT_DISPLAY_ABANDON_SWAPCHAIN;
    }

    context->Processor = processor;
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
NTSTATUS RsIddUnassignSwapChain(IDDCX_MONITOR Monitor)
{
    RsIddMonitorContext* context = RsIddGetMonitorContext(Monitor);
    if (context == nullptr)
    {
        return STATUS_INVALID_PARAMETER;
    }

    delete context->Processor;
    context->Processor = nullptr;
    return STATUS_SUCCESS;
}
