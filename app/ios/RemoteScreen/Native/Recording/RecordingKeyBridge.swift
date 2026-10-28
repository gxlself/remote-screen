import Foundation

enum RecordingKeyBridge {
    private static let recordingWrapCallback: RsRecordingWrapFn = {
        op,
        input,
        inputLength,
        output,
        outputLength in
        guard let outputLength else {
            return RecordingWrapCall.tooSmall
        }
        guard inputLength >= 0, inputLength <= 64 else {
            return RecordingWrapCall.rejected
        }
        guard inputLength == 0 || input != nil else {
            return RecordingWrapCall.rejected
        }

        let inputData: Data
        if let input {
            inputData = Data(bytes: input, count: inputLength)
        } else {
            inputData = Data()
        }

        let (code, result) = RecordingWrapCall.handle(
            op: op,
            input: inputData,
            capacity: outputLength.pointee,
            master: { RecordingKeyStore.shared.master() }
        )
        guard code == RecordingWrapCall.ok else {
            return code
        }
        guard let output else {
            return RecordingWrapCall.tooSmall
        }

        result.copyBytes(to: output, count: result.count)
        outputLength.pointee = result.count
        return RecordingWrapCall.ok
    }

    static func registerAtLaunch() {
        rs_recording_register_wrapper(recordingWrapCallback)
        RsLog.marker("RECKEY", ["registered": true])
    }
}
