#ifndef RS_TAILNET_H
#define RS_TAILNET_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

#define RS_TS_ABI_VERSION 1

/* 返回码：>= 0 成功（或 fd / 字节数），< 0 为下列错误。与 Go internal/node.Code 一一对应。 */
#define RS_TS_OK               0
#define RS_TS_ERR_INVALID     -1 /* 参数非法 */
#define RS_TS_ERR_STATE       -2 /* 未 start / 重复 start */
#define RS_TS_ERR_NOT_RUNNING -3 /* 已 start，未到 Running（等登录 / 正在连） */
#define RS_TS_ERR_TIMEOUT     -4
#define RS_TS_ERR_REFUSED     -5
#define RS_TS_ERR_UNREACHABLE -6 /* 无路由 / 名字解析失败 */
#define RS_TS_ERR_IO          -7
#define RS_TS_ERR_CLOSED      -8 /* 事件流已结束 */

/* 约定：
 * - 全部函数线程安全，可在任意线程并发调用；进程内只有一个节点。
 * - 传入的字符串为 NUL 结尾 UTF-8，Go 侧复制后不再持有。
 * - err / err_cap：可为 NULL / 0；否则写入 NUL 结尾的错误说明（超长截断）。
 * - 输出缓冲区（buf / cap）：返回值 L > 0 表示内容字节数；L <= cap 时已写入（不含 NUL），
 *   L > cap 时未写入、未消费，调用方扩容后重试。
 * - 返回的 fd 是新的、CLOEXEC、阻塞模式，所有权交给调用方，由调用方关闭。
 * - 函数内部 panic 一律转成 RS_TS_ERR_IO，不会让进程崩溃。 */

int32_t rs_ts_abi_version(void);
/* config_json 见 Go internal/node.Config；非阻塞，不等 Running。 */
int32_t rs_ts_start(const char *config_json, char *err, size_t err_cap);
/* 幂等；保留登录状态。 */
int32_t rs_ts_stop(void);
int32_t rs_ts_login(char *err, size_t err_cap);
int32_t rs_ts_logout(char *err, size_t err_cap);
int32_t rs_ts_rebind(void);
/* 状态 JSON（见 「状态 JSON」）。未 start 也返回 state=Stopped 的 JSON。 */
int64_t rs_ts_status(char *buf, size_t cap);
/* 取一条事件 JSON（见 「事件 JSON」）。0 = 超时；RS_TS_ERR_CLOSED = 已结束。
 * timeout_ms 夹到 [0, 60000]。 */
int64_t rs_ts_next_event(char *buf, size_t cap, int32_t timeout_ms);
/* ip 必须是 IP 字面量（tailnet 地址）；返回 AF_UNIX/SOCK_DGRAM fd，只通向 ip:port。
 * 收到 0 字节报文 = 中继已关闭。timeout_ms <= 0 用默认 5000。 */
int32_t rs_ts_dial_udp(const char *ip, uint16_t port, int32_t timeout_ms, char *err, size_t err_cap);
/* host 为 IP 或 MagicDNS 名（短名 / FQDN 均可）；返回 AF_UNIX/SOCK_STREAM fd。
 * timeout_ms <= 0 用默认 10000。 */
int32_t rs_ts_dial_tcp(const char *host, uint16_t port, int32_t timeout_ms, char *err, size_t err_cap);

#ifdef __cplusplus
}
#endif
#endif
