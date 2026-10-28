import Foundation

public struct TerminalRGB: Equatable, Sendable {
    public let red: UInt8
    public let green: UInt8
    public let blue: UInt8

    public init(red: UInt8, green: UInt8, blue: UInt8) {
        self.red = red
        self.green = green
        self.blue = blue
    }
}

public struct TerminalTheme: Equatable, Sendable {
    public let foreground: TerminalRGB
    public let background: TerminalRGB
    public let cursor: TerminalRGB
    public let ansi: [TerminalRGB]

    public init(foreground: TerminalRGB, background: TerminalRGB,
                cursor: TerminalRGB, ansi: [TerminalRGB]) {
        self.foreground = foreground
        self.background = background
        self.cursor = cursor
        self.ansi = ansi
    }

    public static let night = TerminalTheme(
        foreground: rgb("#D5DCE0"),
        background: rgb("#07090B"),
        cursor: rgb("#ECF0F2"),
        ansi: [
            rgb("#1C2328"), rgb("#FF8873"), rgb("#5DD39A"), rgb("#F4B24E"),
            rgb("#7DA8FF"), rgb("#D69CFF"), rgb("#63D7D7"), rgb("#D5DCE0"),
            rgb("#86929A"), rgb("#FF8873"), rgb("#5DD39A"), rgb("#F4B24E"),
            rgb("#7DA8FF"), rgb("#E1B5FF"), rgb("#79E8E8"), rgb("#F4F7F8"),
        ])

    public static func rgb(_ hex: String) -> TerminalRGB {
        let value = hex.trimmingCharacters(in: .whitespacesAndNewlines)
            .drop { $0 == "#" }
        guard value.count == 6, let number = UInt32(value, radix: 16) else {
            return TerminalRGB(red: 0, green: 0, blue: 0)
        }
        return TerminalRGB(red: UInt8((number >> 16) & 0xff),
                          green: UInt8((number >> 8) & 0xff),
                          blue: UInt8(number & 0xff))
    }
}
