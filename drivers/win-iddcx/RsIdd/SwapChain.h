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

#pragma once

#include "RsIdd.h"

#include <avrt.h>
#include <d3d11.h>
#include <dxgi1_5.h>
#include <wrl/client.h>

#include <memory>

class RsIddD3DDevice
{
public:
    explicit RsIddD3DDevice(LUID adapterLuid);

    HRESULT Initialize();

    Microsoft::WRL::ComPtr<ID3D11Device> Device;

private:
    LUID m_adapterLuid;
    Microsoft::WRL::ComPtr<IDXGIFactory5> m_dxgiFactory;
    Microsoft::WRL::ComPtr<IDXGIAdapter1> m_adapter;
    Microsoft::WRL::ComPtr<ID3D11DeviceContext> m_deviceContext;
};

class RsIddSwapChainProcessor
{
public:
    RsIddSwapChainProcessor(
        IDDCX_SWAPCHAIN swapChain,
        std::shared_ptr<RsIddD3DDevice> device,
        HANDLE nextSurfaceAvailable);
    ~RsIddSwapChainProcessor();

private:
    static DWORD WINAPI RunThread(_In_ LPVOID argument);

    void Run();
    void RunCore();

    IDDCX_SWAPCHAIN m_swapChain;
    std::shared_ptr<RsIddD3DDevice> m_device;
    HANDLE m_nextSurfaceAvailable;
    HANDLE m_terminateEvent;
    HANDLE m_thread;
};

EVT_IDD_CX_MONITOR_ASSIGN_SWAPCHAIN RsIddAssignSwapChain;
EVT_IDD_CX_MONITOR_UNASSIGN_SWAPCHAIN RsIddUnassignSwapChain;
