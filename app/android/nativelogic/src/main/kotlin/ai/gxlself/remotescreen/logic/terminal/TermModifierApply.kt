package ai.gxlself.remotescreen.logic.terminal

import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.nio.charset.CodingErrorAction
import java.nio.charset.StandardCharsets

object TermModifierApply {
    data class Result(val bytes: ByteArray, val consumed: Boolean)

    fun apply(bytes: ByteArray, mods: Int): Result {
        val modifiers = mods and (TermMods.CTRL or TermMods.ALT)
        if (modifiers == 0 || bytes.isEmpty() || !isSingleCodePoint(bytes)) {
            return Result(bytes, false)
        }

        var value = bytes
        var changed = false
        if (modifiers and TermMods.CTRL != 0 && bytes.size == 1) {
            controlByte(bytes[0].toInt() and 0xff)?.let {
                value = byteArrayOf(it)
                changed = true
            }
        }
        if (modifiers and TermMods.ALT != 0) {
            return Result(byteArrayOf(ESC) + value, true)
        }
        return Result(value, changed)
    }

    private fun isSingleCodePoint(bytes: ByteArray): Boolean {
        val decoder =
            StandardCharsets.UTF_8
                .newDecoder()
                .onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT)
        return try {
            val text = decoder.decode(ByteBuffer.wrap(bytes)).toString()
            text.codePointCount(0, text.length) == 1
        } catch (_: CharacterCodingException) {
            false
        }
    }

    private fun controlByte(byte: Int): Byte? =
        when (byte) {
            in 0x61..0x7a, in 0x41..0x5a -> (byte and 0x1f).toByte()
            0x40, 0x20 -> 0x00
            0x5b -> 0x1b
            0x5c -> 0x1c
            0x5d -> 0x1d
            0x5e -> 0x1e
            0x5f -> 0x1f
            0x3f -> 0x7f
            else -> null
        }

    private const val ESC: Byte = 0x1b
}
