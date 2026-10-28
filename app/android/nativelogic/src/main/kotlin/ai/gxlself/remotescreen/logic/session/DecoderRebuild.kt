package ai.gxlself.remotescreen.logic.session

enum class DecoderAction {
    Feed,
    Create,
    Rebuild,
}

class KeyframeRequiredException(message: String) : IllegalArgumentException(message)

/** Decides whether a frame can feed the current decoder or requires a new codec. */
fun decoderAction(
    hasCodec: Boolean,
    currentEpoch: Int?,
    frameEpoch: Int,
    currentCodec: Int,
    frameCodec: Int,
    keyframe: Boolean,
    setsChanged: Boolean?,
): DecoderAction {
    val needsDecoder = !hasCodec ||
        currentEpoch != frameEpoch ||
        currentCodec != frameCodec ||
        setsChanged == true
    if (!needsDecoder) return DecoderAction.Feed
    if (!keyframe || setsChanged == null) {
        throw KeyframeRequiredException("keyframe with parameter sets required")
    }
    return if (hasCodec) DecoderAction.Rebuild else DecoderAction.Create
}
