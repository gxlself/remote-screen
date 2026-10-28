#pragma once

#include <cstddef>
#include <cstdint>
#include <cstring>

namespace rsjni {

// Copy a DER signature into the Rust-provided output buffer. The input length
// is the signature length; *len is the output capacity on entry and the actual
// length on success.
inline int32_t writeSignature(const uint8_t* sig,
                              size_t sigLen,
                              uint8_t* out,
                              size_t* len) {
  if (out == nullptr || len == nullptr) {
    return -3;
  }
  if (sigLen == 0 || sig == nullptr) {
    return -1;
  }
  if (*len < sigLen) {
    *len = sigLen;
    return -2;
  }
  std::memcpy(out, sig, sigLen);
  *len = sigLen;
  return 0;
}

}  // namespace rsjni
