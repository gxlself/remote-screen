#pragma once

#include "RsIddIoctl.h"
#include <stddef.h>

namespace rsidd {

constexpr UINT32 kMaxLong = 3840;
constexpr UINT32 kMaxShort = 2560;
constexpr UINT32 kMinLong = 640;
constexpr UINT32 kMinShort = 480;
constexpr UINT32 kMaxMonitorId = 99;

inline bool ModeValid(const RSIDD_MODE& m)
{
    const UINT32 longEdge = m.width > m.height ? m.width : m.height;
    const UINT32 shortEdge = m.width < m.height ? m.width : m.height;
    return longEdge >= kMinLong && longEdge <= kMaxLong &&
        shortEdge >= kMinShort && shortEdge <= kMaxShort &&
        (m.refresh_hz == 30 || m.refresh_hz == 60);
}

inline bool EdidChecksumOk(const BYTE* edid, size_t len)
{
    if (edid == nullptr || len != RSIDD_EDID_LEN) {
        return false;
    }

    UINT32 sum = 0;
    for (size_t i = 0; i < len; ++i) {
        sum += edid[i];
    }
    return sum % 256 == 0;
}

inline UINT32 EdidSerial(const BYTE* edid)
{
    return static_cast<UINT32>(edid[12]) |
        (static_cast<UINT32>(edid[13]) << 8) |
        (static_cast<UINT32>(edid[14]) << 16) |
        (static_cast<UINT32>(edid[15]) << 24);
}

enum class AddCheck { Ok, BadVersion, BadId, BadModes, BadEdid };

inline AddCheck CheckAddIn(const RSIDD_ADD_IN& in)
{
    if (in.version != RSIDD_VERSION) {
        return AddCheck::BadVersion;
    }
    if (in.monitor_id == 0 || in.monitor_id > kMaxMonitorId) {
        return AddCheck::BadId;
    }
    if (in.mode_count == 0 || in.mode_count > RSIDD_MAX_MODES) {
        return AddCheck::BadModes;
    }
    for (UINT32 i = 0; i < in.mode_count; ++i) {
        if (!ModeValid(in.modes[i])) {
            return AddCheck::BadModes;
        }
    }
    if (!EdidChecksumOk(in.edid, in.edid_len) || EdidSerial(in.edid) != in.monitor_id) {
        return AddCheck::BadEdid;
    }
    return AddCheck::Ok;
}

inline int FreeSlot(const bool inUse[], size_t n)
{
    for (size_t i = 0; i < n; ++i) {
        if (!inUse[i]) {
            return static_cast<int>(i);
        }
    }
    return -1;
}

inline void FillInfo(RSIDD_INFO* info, UINT32 active)
{
    *info = {};
    info->version = RSIDD_VERSION;
    info->max_monitors = 4;
    info->max_long_px = kMaxLong;
    info->max_short_px = kMaxShort;
    info->min_long_px = kMinLong;
    info->min_short_px = kMinShort;
    info->refresh_count = 2;
    info->refresh[0] = 30;
    info->refresh[1] = 60;
    info->active_monitors = active;
}

} // namespace rsidd
