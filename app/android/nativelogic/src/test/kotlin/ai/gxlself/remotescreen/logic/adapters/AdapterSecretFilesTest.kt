package ai.gxlself.remotescreen.logic.adapters

import java.io.File
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.concurrent.Executors
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class AdapterSecretFilesTest {
  @get:Rule val temporary = TemporaryFolder()

  private val random = SecureRandom()

  private fun store(
    directory: File = temporary.newFolder(),
    cipher: AdapterSecretCipher = aesCipher(),
  ): AdapterSecretFileStore = AdapterSecretFileStore(directory, cipher)

  private fun aesCipher(): AdapterSecretCipher {
    val keyBytes = ByteArray(32).also(random::nextBytes)
    val key = SecretKeySpec(keyBytes, "AES")
    return AdapterSecretCipher(
      encrypt = { plain, aad ->
        val iv = ByteArray(12).also(random::nextBytes)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key, GCMParameterSpec(128, iv))
        cipher.updateAAD(aad)
        iv to cipher.doFinal(plain)
      },
      decrypt = { iv, ciphertext, aad ->
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, iv))
        cipher.updateAAD(aad)
        try {
          cipher.doFinal(ciphertext)
        } catch (_: AEADBadTagException) {
          null
        }
      },
    )
  }

  @Test
  fun validKey() {
    assertTrue(AdapterSecretFormat.validKey("vnc:h1".toByteArray()))
    assertFalse(AdapterSecretFormat.validKey(ByteArray(0)))
    assertFalse(AdapterSecretFormat.validKey(ByteArray(301)))
    assertFalse(AdapterSecretFormat.validKey(byteArrayOf(0x76, 0x6e, 0x00, 0x63)))
    assertFalse(AdapterSecretFormat.validKey(byteArrayOf(0xc3.toByte(), 0x28)))
  }

  @Test
  fun fileName() {
    val key = "vnc:h1".toByteArray()
    val expectedDigest = MessageDigest.getInstance("SHA-256").digest(key)
      .joinToString("") { "%02x".format(it.toInt() and 0xff) }
    val result = AdapterSecretFormat.fileName(key)
    assertEquals("$expectedDigest.bin", result)
    assertTrue(result.matches(Regex("[0-9a-f]+\\.bin")))
    assertEquals(68, result.length)
  }

  @Test
  fun packUnpack() {
    val iv = ByteArray(12) { it.toByte() }
    val ciphertext = ByteArray(16) { (it + 20).toByte() }
    val blob = AdapterSecretFormat.pack(iv, ciphertext)
    assertEquals(1, blob[0].toInt())
    val unpacked = AdapterSecretFormat.unpack(blob)
    assertNotNull(unpacked)
    assertArrayEquals(iv, unpacked!!.first)
    assertArrayEquals(ciphertext, unpacked.second)
    assertEquals(null, AdapterSecretFormat.unpack(ByteArray(28) { 1 }))
    try {
      AdapterSecretFormat.pack(ByteArray(11), ciphertext)
      fail("Expected invalid IV")
    } catch (_: IllegalArgumentException) {
      // Expected.
    }
  }

  @Test
  fun roundtrip() {
    val directory = temporary.newFolder()
    val fileStore = store(directory)
    val key = "rdp:192.168.31.20:3389".toByteArray()
    val value = "{\"v\":1,\"user\":\"alice\",\"password\":\"secret\"}".toByteArray()
    assertTrue(fileStore.put(key, value))
    val result = fileStore.get(key)
    assertTrue(result is AdapterSecretFileStore.Get.Found)
    assertArrayEquals(value, (result as AdapterSecretFileStore.Get.Found).value)

    val encrypted = File(directory, AdapterSecretFormat.fileName(key)).readBytes()
    for (start in 0..value.size - 8) {
      assertFalse(encrypted.toList().windowed(8).any { window ->
        window == value.slice(start until start + 8)
      })
    }
  }

  @Test
  fun missing() {
    val fileStore = store()
    val key = "vnc:missing".toByteArray()
    assertTrue(fileStore.get(key) is AdapterSecretFileStore.Get.Missing)
    assertTrue(fileStore.delete(key))
  }

  @Test
  fun overwrite() {
    val directory = temporary.newFolder()
    val fileStore = store(directory)
    val key = "vnc:overwrite".toByteArray()
    assertTrue(fileStore.put(key, "first".toByteArray()))
    assertTrue(fileStore.put(key, "second".toByteArray()))
    assertArrayEquals(
      "second".toByteArray(),
      (fileStore.get(key) as AdapterSecretFileStore.Get.Found).value,
    )
    assertEquals(1, directory.listFiles { file -> file.extension == "bin" }?.size)
    assertTrue(directory.listFiles { file -> file.name.endsWith(".tmp") }.orEmpty().isEmpty())
  }

  @Test
  fun swappedFile() {
    val directory = temporary.newFolder()
    val fileStore = store(directory)
    val keyA = "vnc:a".toByteArray()
    val keyB = "vnc:b".toByteArray()
    assertTrue(fileStore.put(keyA, "value".toByteArray()))
    File(directory, AdapterSecretFormat.fileName(keyA)).copyTo(
      File(directory, AdapterSecretFormat.fileName(keyB)),
      overwrite = true,
    )
    assertTrue(fileStore.get(keyB) is AdapterSecretFileStore.Get.Missing)
    assertFalse(File(directory, AdapterSecretFormat.fileName(keyB)).exists())
  }

  @Test
  fun corruptFile() {
    val directory = temporary.newFolder()
    val fileStore = store(directory)
    val key = "vnc:corrupt".toByteArray()
    assertTrue(fileStore.put(key, "value".toByteArray()))
    val file = File(directory, AdapterSecretFormat.fileName(key))
    val bytes = file.readBytes()
    bytes[bytes.lastIndex] = (bytes.last().toInt() xor 1).toByte()
    file.writeBytes(bytes)
    assertTrue(fileStore.get(key) is AdapterSecretFileStore.Get.Missing)
    assertFalse(file.exists())
  }

  @Test
  fun cipherUnavailable() {
    val directory = temporary.newFolder()
    val key = "vnc:unavailable".toByteArray()
    val normal = store(directory)
    assertTrue(normal.put(key, "value".toByteArray()))
    val unavailable = store(directory, AdapterSecretCipher(
      encrypt = { _, _ -> error("unavailable") },
      decrypt = { _, _, _ -> error("unavailable") },
    ))
    assertTrue(unavailable.get(key) is AdapterSecretFileStore.Get.Failed)
    assertTrue(File(directory, AdapterSecretFormat.fileName(key)).exists())

    val encryptFailureDirectory = temporary.newFolder()
    val encryptFailure = store(encryptFailureDirectory, AdapterSecretCipher(
      encrypt = { _, _ -> error("unavailable") },
      decrypt = { _, _, _ -> null },
    ))
    assertFalse(encryptFailure.put(key, "value".toByteArray()))
    assertTrue(encryptFailureDirectory.listFiles().orEmpty().isEmpty())
  }

  @Test
  fun limits() {
    val fileStore = store()
    val key = "vnc:limits".toByteArray()
    assertTrue(fileStore.put(key, ByteArray(4096) { 1 }))
    assertFalse(fileStore.put(key, ByteArray(4097) { 1 }))
    assertFalse(fileStore.put(key, ByteArray(0)))
  }

  @Test
  fun concurrent() {
    val directory = temporary.newFolder()
    val fileStore = store(directory)
    val keys = (0 until 4).map { "vnc:concurrent:$it".toByteArray() }
    val executor = Executors.newFixedThreadPool(8)
    try {
      val futures = (0 until 8).map { thread ->
        executor.submit {
          repeat(50) { iteration ->
            val keyIndex = (thread + iteration) % keys.size
            val key = keys[keyIndex]
            val value = "thread=$thread;iteration=$iteration".toByteArray()
            assertTrue(fileStore.put(key, value))
            val result = fileStore.get(key)
            assertTrue(result is AdapterSecretFileStore.Get.Found)
            val actual = (result as AdapterSecretFileStore.Get.Found).value
            assertTrue(actual.decodeToString().matches(Regex("thread=[0-7];iteration=[0-9]+")))
          }
        }
      }
      futures.forEach { it.get() }
    } finally {
      executor.shutdownNow()
    }
  }
}
