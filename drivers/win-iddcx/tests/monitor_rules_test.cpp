#include "MonitorRules.h"

#include <array>
#include <stdio.h>
#include <string.h>

static_assert(sizeof(RSIDD_ADD_IN) == 192, "RSIDD_ADD_IN size");
static_assert(offsetof(RSIDD_ADD_IN, edid) == 64, "RSIDD_ADD_IN EDID offset");

#define CHECK(cond) do { \
    if (!(cond)) { \
        fprintf(stderr, "CHECK failed at line %d: %s\n", __LINE__, #cond); \
        return 1; \
    } \
} while (false)

static std::array<BYTE, RSIDD_EDID_LEN> make_edid(UINT32 id)
{
    std::array<BYTE, RSIDD_EDID_LEN> edid = {};
    for (size_t i = 0; i < 4; ++i) {
        edid[12 + i] = static_cast<BYTE>(id >> (8 * i));
    }
    UINT32 sum = 0;
    for (size_t i = 0; i < edid.size() - 1; ++i) {
        sum += edid[i];
    }
    edid.back() = static_cast<BYTE>((256 - sum % 256) % 256);
    return edid;
}

static RSIDD_ADD_IN make_add(UINT32 id)
{
    RSIDD_ADD_IN in = {};
    in.version = RSIDD_VERSION;
    in.monitor_id = id;
    in.mode_count = 2;
    in.modes[0] = {2732, 2048, 60};
    in.modes[1] = {2048, 2732, 30};
    in.edid_len = RSIDD_EDID_LEN;
    const auto edid = make_edid(id);
    memcpy(in.edid, edid.data(), edid.size());
    return in;
}

int main()
{
    CHECK(rsidd::ModeValid({2732, 2048, 60}));
    CHECK(rsidd::ModeValid({2048, 2732, 30}));
    CHECK(rsidd::ModeValid({3840, 2560, 60}));
    CHECK(rsidd::ModeValid({640, 480, 60}));
    CHECK(rsidd::ModeValid({2560, 3840, 30}));
    CHECK(rsidd::ModeValid({480, 640, 30}));
    CHECK(!rsidd::ModeValid({3848, 2048, 60}));
    CHECK(!rsidd::ModeValid({2732, 2568, 60}));
    CHECK(!rsidd::ModeValid({632, 480, 60}));
    CHECK(!rsidd::ModeValid({640, 472, 60}));
    CHECK(!rsidd::ModeValid({1920, 1080, 120}));
    CHECK(!rsidd::ModeValid({1920, 1080, 0}));
    CHECK(!rsidd::ModeValid({0, 0, 60}));
    CHECK(!rsidd::ModeValid({0xffffffffu, 1080, 60}));

    auto edid = make_edid(1);
    CHECK(rsidd::EdidChecksumOk(edid.data(), edid.size()));
    edid[20] ^= 1;
    CHECK(!rsidd::EdidChecksumOk(edid.data(), edid.size()));
    edid = make_edid(1);
    CHECK(!rsidd::EdidChecksumOk(edid.data(), 127));
    CHECK(!rsidd::EdidChecksumOk(edid.data(), 129));
    CHECK(!rsidd::EdidChecksumOk(nullptr, RSIDD_EDID_LEN));
    edid = make_edid(42);
    CHECK(rsidd::EdidSerial(edid.data()) == 42);
    edid = make_edid(0x01020304);
    CHECK(edid[12] == 0x04 && edid[13] == 0x03 && edid[14] == 0x02 && edid[15] == 0x01);
    CHECK(rsidd::EdidSerial(edid.data()) == 0x01020304);
    edid = make_edid(0xff020304u);
    CHECK(rsidd::EdidSerial(edid.data()) == 0xff020304u);

    using rsidd::AddCheck;
    auto in = make_add(42);
    CHECK(rsidd::CheckAddIn(in) == AddCheck::Ok);
    CHECK(rsidd::CheckAddIn(make_add(1)) == AddCheck::Ok);
    CHECK(rsidd::CheckAddIn(make_add(99)) == AddCheck::Ok);
    in.version = 2;
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadVersion);
    in = make_add(42);
    in.monitor_id = 0;
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadId);
    in.monitor_id = 100;
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadId);
    in = make_add(42);
    in.mode_count = 0;
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadModes);
    in.mode_count = 5;
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadModes);
    in.mode_count = 2;
    in.modes[1].refresh_hz = 120;
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadModes);
    in.mode_count = 1;
    CHECK(rsidd::CheckAddIn(in) == AddCheck::Ok);
    in = make_add(42);
    in.mode_count = RSIDD_MAX_MODES;
    in.modes[2] = {3840, 2560, 60};
    in.modes[3] = {640, 480, 30};
    CHECK(rsidd::CheckAddIn(in) == AddCheck::Ok);
    in.modes[3].width = 632;
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadModes);
    in = make_add(42);
    in.edid_len = 127;
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadEdid);
    in.edid_len = 129;
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadEdid);
    in = make_add(42);
    in.edid[20] ^= 1;
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadEdid);
    in = make_add(42);
    edid = make_edid(43);
    memcpy(in.edid, edid.data(), edid.size());
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadEdid);

    // Invalid fields must be reported in protocol order.
    in = {};
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadVersion);
    in.version = RSIDD_VERSION;
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadId);
    in.monitor_id = 1;
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadModes);
    in.mode_count = 1;
    in.modes[0] = {640, 480, 60};
    CHECK(rsidd::CheckAddIn(in) == AddCheck::BadEdid);

    const bool empty[] = {false, false, false, false};
    const bool partial[] = {true, false, true, false};
    const bool full[] = {true, true, true, true};
    CHECK(rsidd::FreeSlot(empty, 4) == 0);
    CHECK(rsidd::FreeSlot(partial, 4) == 1);
    CHECK(rsidd::FreeSlot(full, 4) == -1);
    CHECK(rsidd::FreeSlot(nullptr, 0) == -1);

    RSIDD_INFO info;
    memset(&info, 0xff, sizeof(info));
    rsidd::FillInfo(&info, 2);
    CHECK(info.version == RSIDD_VERSION);
    CHECK(info.max_monitors == 4);
    CHECK(info.max_long_px == 3840);
    CHECK(info.max_short_px == 2560);
    CHECK(info.min_long_px == 640);
    CHECK(info.min_short_px == 480);
    CHECK(info.refresh_count == 2);
    CHECK(info.refresh[0] == 30);
    CHECK(info.refresh[1] == 60);
    CHECK(info.refresh[2] == 0);
    CHECK(info.refresh[3] == 0);
    CHECK(info.active_monitors == 2);
    rsidd::FillInfo(&info, 0);
    CHECK(info.active_monitors == 0);
    rsidd::FillInfo(&info, 4);
    CHECK(info.active_monitors == 4);

    puts("ALL OK");
    return 0;
}
