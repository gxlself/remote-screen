package ai.gxlself.remotescreen.logic.gestures

data class GPoint(val x: Float, val y: Float) {
    companion object {
        val ZERO = GPoint(0f, 0f)
    }
}

data class GRect(val x: Float, val y: Float, val w: Float, val h: Float) {
    companion object {
        val ZERO = GRect(0f, 0f, 0f, 0f)
    }
}
