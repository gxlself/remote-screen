# NativeLogic

`NativeLogic` contains platform-neutral Swift logic shared by the iOS app and
the standalone SwiftPM test target. Keep this package limited to Foundation,
CryptoKit, CoreGraphics, and Network. Do not import UIKit, AVFoundation,
VideoToolbox, or React.

Types use a feature prefix because these sources are also compiled directly
into the `RemoteScreen` app target. Keep declarations `internal` by default
and use `@testable import NativeLogic` from tests.

Each feature gets a directory under
`Sources/NativeLogic/<feature>/` and a matching test file under
`Tests/NativeLogicTests/<feature>Tests.swift`. The iOS project is updated by
`scripts/configure-project.rb`; do not hand-edit its project file.
