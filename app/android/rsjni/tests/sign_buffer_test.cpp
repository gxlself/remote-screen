#include "rs_sign_buffer.h"

#include <array>
#include <cassert>
#include <cstdint>

int main() {
  const std::array<uint8_t, 4> signature{1, 2, 3, 4};
  std::array<uint8_t, 4> output{};
  size_t length = output.size();

  assert(rsjni::writeSignature(signature.data(), signature.size(), output.data(), &length) == 0);
  assert(length == signature.size());
  assert(output == signature);

  std::array<uint8_t, 3> shortOutput{};
  length = shortOutput.size();
  assert(rsjni::writeSignature(signature.data(), signature.size(), shortOutput.data(), &length) == -2);
  assert(length == signature.size());

  length = output.size();
  assert(rsjni::writeSignature(signature.data(), signature.size(), nullptr, &length) == -3);
  assert(rsjni::writeSignature(signature.data(), signature.size(), output.data(), nullptr) == -3);

  length = output.size();
  assert(rsjni::writeSignature(signature.data(), 0, output.data(), &length) == -1);
  assert(rsjni::writeSignature(nullptr, signature.size(), output.data(), &length) == -1);
  return 0;
}
