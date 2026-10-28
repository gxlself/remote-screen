package ai.gxlself.remotescreen.rsnative.session.video

import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.os.Build
import ai.gxlself.remotescreen.logic.session.CodecCandidate
import ai.gxlself.remotescreen.logic.session.DecoderChoice

internal object CodecPicker {
  fun pick(mime: String): CodecCandidate? {
    val candidates = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.mapNotNull { info ->
      if (info.isEncoder || info.supportedTypes.none { it.equals(mime, ignoreCase = true) }) {
        return@mapNotNull null
      }
      // Broken vendor capability entries must not hide the remaining decoders.
      try {
        val capabilities = info.getCapabilitiesForType(mime)
        val software = if (Build.VERSION.SDK_INT >= 29) {
          info.isSoftwareOnly
        } else {
          info.name.startsWith("OMX.google.", ignoreCase = true) ||
            info.name.startsWith("c2.android.", ignoreCase = true)
        }
        CodecCandidate(
          name = info.name,
          hardware = if (Build.VERSION.SDK_INT >= 29) info.isHardwareAccelerated else !software,
          lowLatencyFeature = Build.VERSION.SDK_INT >= 30 &&
            capabilities.isFeatureSupported(MediaCodecInfo.CodecCapabilities.FEATURE_LowLatency),
          softwareOnly = software,
        )
      } catch (_: IllegalArgumentException) {
        null
      }
    }
    return DecoderChoice.choose(candidates)
  }
}
