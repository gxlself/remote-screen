// Remote Screen 虚拟显示器驱动：各源文件共用的声明。
// 部分代码改编自 Microsoft Windows-driver-samples video/IndirectDisplay（MIT License）。
#pragma once
#include <windows.h>
#include <wdf.h>
#include <IddCx.h>
#include "..\include\RsIddIoctl.h"

#define RSIDD_MAX_MONITORS 4u

class RsIddSwapChainProcessor;   // SwapChain.h

struct RsIddMonitorEntry {
    BOOLEAN       InUse;
    UINT32        MonitorId;        // = EDID 序列号（字节 12..15，小端）
    UINT32        ConnectorIndex;   // 0..RSIDD_MAX_MONITORS-1
    IDDCX_MONITOR Monitor;          // IddCxMonitorCreate 之后才有效
    UINT32        ModeCount;        // 1..RSIDD_MAX_MODES
    RSIDD_MODE    Modes[RSIDD_MAX_MODES];
    BYTE          Edid[RSIDD_EDID_LEN];
    GUID          ContainerId;
};

struct RsIddDeviceContext {
    WDFDEVICE         Device;
    IDDCX_ADAPTER     Adapter;
    BOOLEAN           AdapterReady;     // AdapterInitFinished 成功后为 TRUE
    WDFWAITLOCK       Lock;             // 只保护 Monitors[]；持锁期间不得调用任何 IddCx 函数
    RsIddMonitorEntry Monitors[RSIDD_MAX_MONITORS];
    volatile LONGLONG LastControlTick;  // GetTickCount64，最后一次收到 IOCTL 的时刻（Control.cpp 读写）
    WDFTIMER          LeaseTimer;       // Control.cpp 创建
};
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(RsIddDeviceContext, RsIddGetContext);

struct RsIddMonitorContext {
    RsIddDeviceContext*      Device;
    UINT32                   MonitorId;
    RsIddSwapChainProcessor* Processor;   // SwapChain.cpp 管理
};
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(RsIddMonitorContext, RsIddGetMonitorContext);

// Root 枚举只有一个设备；ParseMonitorDescription 回调没有设备参数，经它找到显示器表。Driver.cpp 定义。
extern RsIddDeviceContext* g_RsIddDevice;

// ---- Monitors.cpp----
NTSTATUS RsIddAddMonitor(RsIddDeviceContext* Ctx, const RSIDD_ADD_IN* In, RSIDD_ADD_OUT* Out);
NTSTATUS RsIddRemoveMonitor(RsIddDeviceContext* Ctx, UINT32 MonitorId);
UINT32   RsIddRemoveAllMonitors(RsIddDeviceContext* Ctx);
void     RsIddFillInfo(RsIddDeviceContext* Ctx, RSIDD_INFO* Info);
EVT_IDD_CX_PARSE_MONITOR_DESCRIPTION             RsIddParseMonitorDescription;
EVT_IDD_CX_MONITOR_GET_DEFAULT_DESCRIPTION_MODES RsIddMonitorGetDefaultModes;
EVT_IDD_CX_MONITOR_QUERY_TARGET_MODES            RsIddMonitorQueryModes;

// ---- Control.cpp----
EVT_IDD_CX_DEVICE_IO_CONTROL RsIddEvtDeviceIoControl;
NTSTATUS RsIddControlStart(RsIddDeviceContext* Ctx);   // AdapterInitFinished 成功后调用：建 1 秒的租约定时器
void     RsIddControlStop(RsIddDeviceContext* Ctx);    // D0Exit 时调用

// ---- Driver.cpp / SwapChain.cpp：DriverEntry、EvtDeviceAdd、D0Entry/D0Exit、
//      AdapterInitFinished、AdapterCommitModes 是 Driver.cpp 内 static；
//      AssignSwapChain / UnassignSwapChain 声明在 SwapChain.h 里。
