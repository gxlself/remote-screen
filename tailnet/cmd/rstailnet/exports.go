package main

/*
#include <stddef.h>
#include <stdint.h>
*/
import "C"

import (
	"context"
	"fmt"
	"time"
	"unsafe"

	"remotescreen/tailnet/internal/node"
)

var singleton = node.New()

func writeError(buf *C.char, capacity C.size_t, message string) {
	if buf == nil || capacity == 0 {
		return
	}
	length := min(uint64(len(message)), uint64(capacity)-1)
	out := unsafe.Slice((*byte)(unsafe.Pointer(buf)), int(length)+1)
	copy(out, message[:length])
	out[length] = 0
}

func recover32(result *C.int32_t, buf *C.char, capacity C.size_t) {
	if v := recover(); v != nil {
		*result = C.int32_t(node.CodeIO)
		writeError(buf, capacity, fmt.Sprintf("panic: %v", v))
	}
}

func recover64(result *C.int64_t) {
	if recover() != nil {
		*result = C.int64_t(node.CodeIO)
	}
}

func errorResult(err error, buf *C.char, capacity C.size_t) C.int32_t {
	message := ""
	if err != nil {
		message = err.Error()
	}
	writeError(buf, capacity, message)
	return C.int32_t(node.CodeOf(err))
}

func outputCapacity(buf *C.char, capacity C.size_t) int {
	if buf == nil {
		return 0
	}
	return int(min(uint64(capacity), uint64(^uint(0)>>1)))
}

func output(buf *C.char, capacity C.size_t, data []byte) C.int64_t {
	if len(data) <= outputCapacity(buf, capacity) && len(data) != 0 {
		copy(unsafe.Slice((*byte)(unsafe.Pointer(buf)), len(data)), data)
	}
	return C.int64_t(len(data))
}

//export rs_ts_abi_version
func rs_ts_abi_version() (result C.int32_t) {
	defer recover32(&result, nil, 0)
	return 1
}

//export rs_ts_start
func rs_ts_start(configJSON *C.char, errBuf *C.char, errCap C.size_t) (result C.int32_t) {
	defer recover32(&result, errBuf, errCap)
	if configJSON == nil {
		return errorResult(node.Errorf(node.CodeInvalid, "config_json is NULL"), errBuf, errCap)
	}
	cfg, err := node.ParseConfig([]byte(C.GoString(configJSON)))
	if err == nil {
		err = singleton.Start(cfg)
	}
	return errorResult(err, errBuf, errCap)
}

//export rs_ts_stop
func rs_ts_stop() (result C.int32_t) {
	defer recover32(&result, nil, 0)
	return errorResult(singleton.Stop(), nil, 0)
}

//export rs_ts_login
func rs_ts_login(errBuf *C.char, errCap C.size_t) (result C.int32_t) {
	defer recover32(&result, errBuf, errCap)
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	return errorResult(singleton.Login(ctx), errBuf, errCap)
}

//export rs_ts_logout
func rs_ts_logout(errBuf *C.char, errCap C.size_t) (result C.int32_t) {
	defer recover32(&result, errBuf, errCap)
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	return errorResult(singleton.Logout(ctx), errBuf, errCap)
}

//export rs_ts_rebind
func rs_ts_rebind() (result C.int32_t) {
	defer recover32(&result, nil, 0)
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	return errorResult(singleton.Rebind(ctx), nil, 0)
}

//export rs_ts_status
func rs_ts_status(buf *C.char, capacity C.size_t) (result C.int64_t) {
	defer recover64(&result)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	data, err := singleton.StatusJSON(ctx)
	if err != nil {
		return C.int64_t(node.CodeOf(err))
	}
	return output(buf, capacity, data)
}

//export rs_ts_next_event
func rs_ts_next_event(buf *C.char, capacity C.size_t, timeoutMS C.int32_t) (result C.int64_t) {
	defer recover64(&result)
	timeout := time.Duration(max(0, min(timeoutMS, 60000))) * time.Millisecond
	data, need, err := singleton.NextEvent(timeout, outputCapacity(buf, capacity))
	if err != nil {
		return C.int64_t(node.CodeOf(err))
	}
	if data != nil {
		output(buf, capacity, data)
	}
	return C.int64_t(need)
}

//export rs_ts_dial_udp
func rs_ts_dial_udp(ip *C.char, port C.uint16_t, timeoutMS C.int32_t, errBuf *C.char, errCap C.size_t) (result C.int32_t) {
	defer recover32(&result, errBuf, errCap)
	if ip == nil {
		return errorResult(node.Errorf(node.CodeInvalid, "ip is NULL"), errBuf, errCap)
	}
	fd, err := singleton.DialUDP(C.GoString(ip), uint16(port), time.Duration(timeoutMS)*time.Millisecond)
	if code := errorResult(err, errBuf, errCap); code != 0 {
		return code
	}
	return C.int32_t(fd)
}

//export rs_ts_dial_tcp
func rs_ts_dial_tcp(host *C.char, port C.uint16_t, timeoutMS C.int32_t, errBuf *C.char, errCap C.size_t) (result C.int32_t) {
	defer recover32(&result, errBuf, errCap)
	if host == nil {
		return errorResult(node.Errorf(node.CodeInvalid, "host is NULL"), errBuf, errCap)
	}
	fd, err := singleton.DialTCP(C.GoString(host), uint16(port), time.Duration(timeoutMS)*time.Millisecond)
	if code := errorResult(err, errBuf, errCap); code != 0 {
		return code
	}
	return C.int32_t(fd)
}
