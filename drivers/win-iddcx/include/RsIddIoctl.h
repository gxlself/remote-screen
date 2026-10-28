// Remote Screen IddCx virtual display IOCTL protocol.
// Payloads are packed to one byte; every UINT32 field is little-endian.
// EDID is an opaque byte array supplied by the service.
// This header corresponds field-for-field with
// crates/host-win/src/vdisplay_ioctl.rs; change both files together.
// 与 crates/host-win/src/vdisplay_ioctl.rs 逐字段对应，改一处必须改另一处。
// The compilation unit that defines the GUID entity (Driver.cpp) must include
// <initguid.h> before including this header.  <initguid.h> is intentionally not
// included here.
// 需要定义 GUID 实体的那一个编译单元（Driver.cpp）在包含本头之前自行
// #include <initguid.h>。
// GCC diagnoses pragma-once when a header is intentionally compiled as a
// translation unit; retain the pragma for normal inclusion and MSVC builds.
#if !defined(__GNUC__) || defined(_WIN32) || (__INCLUDE_LEVEL__ > 0)
#pragma once
#endif

#ifdef _WIN32
#include <windows.h>
#include <winioctl.h>
#else
#include <stdint.h>
typedef uint32_t UINT32;
typedef uint8_t BYTE;
#define FILE_DEVICE_UNKNOWN 0x00000022u
#define METHOD_BUFFERED 0u
#define FILE_READ_ACCESS 0x0001u
#define FILE_WRITE_ACCESS 0x0002u
#define CTL_CODE(t, f, m, a) (((t) << 16) | ((a) << 14) | ((f) << 2) | (m))
#endif

#ifdef _WIN32
// {13037E20-EC6D-42CD-8293-42F53DA517A6}
DEFINE_GUID(GUID_DEVINTERFACE_RSIDD, 0x13037e20, 0xec6d, 0x42cd, 0x82, 0x93, 0x42, 0xf5, 0x3d, 0xa5, 0x17, 0xa6);
#endif

#define RSIDD_VERSION 1u
#define RSIDD_MAX_MODES 4u
#define RSIDD_EDID_LEN 128u

#define IOCTL_RSIDD_GET_INFO CTL_CODE(FILE_DEVICE_UNKNOWN, 0x900, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS) /* 0x0022E400 */
#define IOCTL_RSIDD_ADD CTL_CODE(FILE_DEVICE_UNKNOWN, 0x901, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS) /* 0x0022E404 */
#define IOCTL_RSIDD_REMOVE CTL_CODE(FILE_DEVICE_UNKNOWN, 0x902, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS) /* 0x0022E408 */
#define IOCTL_RSIDD_REMOVE_ALL CTL_CODE(FILE_DEVICE_UNKNOWN, 0x903, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS) /* 0x0022E40C */
#define IOCTL_RSIDD_PING CTL_CODE(FILE_DEVICE_UNKNOWN, 0x904, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS) /* 0x0022E410 */

#ifdef __cplusplus
#define RSIDD_STATIC_ASSERT(condition, message) static_assert(condition, message)
#else
#define RSIDD_STATIC_ASSERT(condition, message) _Static_assert(condition, message)
#endif

#pragma pack(push, 1)

typedef struct _RSIDD_VERSION_IN {
    UINT32 version;
} RSIDD_VERSION_IN;
RSIDD_STATIC_ASSERT(sizeof(RSIDD_VERSION_IN) == 4, "RSIDD_VERSION_IN size");

typedef struct _RSIDD_MODE {
    UINT32 width;
    UINT32 height;
    UINT32 refresh_hz;
} RSIDD_MODE;
RSIDD_STATIC_ASSERT(sizeof(RSIDD_MODE) == 12, "RSIDD_MODE size");

typedef struct _RSIDD_INFO {
    UINT32 version;
    UINT32 max_monitors;
    UINT32 max_long_px;
    UINT32 max_short_px;
    UINT32 min_long_px;
    UINT32 min_short_px;
    UINT32 refresh_count;
    UINT32 refresh[RSIDD_MAX_MODES];
    UINT32 active_monitors;
} RSIDD_INFO;
RSIDD_STATIC_ASSERT(sizeof(RSIDD_INFO) == 48, "RSIDD_INFO size");

typedef struct _RSIDD_ADD_IN {
    UINT32 version;
    UINT32 monitor_id;
    UINT32 mode_count;
    RSIDD_MODE modes[RSIDD_MAX_MODES];
    UINT32 edid_len;
    BYTE edid[RSIDD_EDID_LEN];
} RSIDD_ADD_IN;
RSIDD_STATIC_ASSERT(sizeof(RSIDD_ADD_IN) == 192, "RSIDD_ADD_IN size");

typedef struct _RSIDD_ADD_OUT {
    UINT32 monitor_id;
    UINT32 connector_index;
} RSIDD_ADD_OUT;
RSIDD_STATIC_ASSERT(sizeof(RSIDD_ADD_OUT) == 8, "RSIDD_ADD_OUT size");

typedef struct _RSIDD_REMOVE_IN {
    UINT32 version;
    UINT32 monitor_id;
} RSIDD_REMOVE_IN;
RSIDD_STATIC_ASSERT(sizeof(RSIDD_REMOVE_IN) == 8, "RSIDD_REMOVE_IN size");

typedef struct _RSIDD_REMOVE_ALL_OUT {
    UINT32 removed;
} RSIDD_REMOVE_ALL_OUT;
RSIDD_STATIC_ASSERT(sizeof(RSIDD_REMOVE_ALL_OUT) == 4, "RSIDD_REMOVE_ALL_OUT size");

#pragma pack(pop)

RSIDD_STATIC_ASSERT(IOCTL_RSIDD_GET_INFO == 0x0022E400u, "GET_INFO IOCTL code");
RSIDD_STATIC_ASSERT(IOCTL_RSIDD_ADD == 0x0022E404u, "ADD IOCTL code");
RSIDD_STATIC_ASSERT(IOCTL_RSIDD_REMOVE == 0x0022E408u, "REMOVE IOCTL code");
RSIDD_STATIC_ASSERT(IOCTL_RSIDD_REMOVE_ALL == 0x0022E40Cu, "REMOVE_ALL IOCTL code");
RSIDD_STATIC_ASSERT(IOCTL_RSIDD_PING == 0x0022E410u, "PING IOCTL code");
#undef RSIDD_STATIC_ASSERT
