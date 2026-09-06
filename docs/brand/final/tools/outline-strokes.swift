// Convert the approved SVG's open strokes into closed, filled vector outlines.
// Icon Composer's older renderer applies fill overrides to open path interiors.
import Foundation
import CoreGraphics

struct InputPath: Decodable {
    let d: String
    let width: Double
}

let inputs = try JSONDecoder().decode([InputPath].self, from: FileHandle.standardInput.readDataToEndOfFile())
let tokenPattern = try NSRegularExpression(pattern: "[A-Za-z]|[-+]?(?:[0-9]*\\.)?[0-9]+(?:[eE][-+]?[0-9]+)?")
func number(_ value: CGFloat) -> String {
    String(format: "%.9f", Double(value)).replacingOccurrences(of: #"\.?0+$"#, with: "", options: .regularExpression)
}
var output: [String] = []
for input in inputs {
    let text = input.d as NSString
    let tokens = tokenPattern.matches(in: input.d, range: NSRange(location: 0, length: text.length)).map { text.substring(with: $0.range) }
    let path = CGMutablePath()
    var index = 0
    func coordinate() -> CGFloat {
        guard index < tokens.count, let value = Double(tokens[index]) else {
            fatalError("Invalid SVG coordinate at token \(index)")
        }
        index += 1
        return CGFloat(value)
    }
    func point() -> CGPoint { CGPoint(x: coordinate(), y: coordinate()) }
    while index < tokens.count {
        let command = tokens[index]
        index += 1
        switch command {
        case "M": path.move(to: point())
        case "L": path.addLine(to: point())
        case "C":
            let c1 = point(), c2 = point(), end = point()
            path.addCurve(to: end, control1: c1, control2: c2)
        case "Z": path.closeSubpath()
        default: fatalError("Unsupported SVG command \(command); do not silently alter the master")
        }
    }
    let outline = path.copy(strokingWithWidth: input.width, lineCap: .butt, lineJoin: .miter, miterLimit: 4)
        .normalized(using: .winding)
    var commands: [String] = []
    outline.applyWithBlock { pointer in
        let element = pointer.pointee
        func p(_ i: Int) -> String { "\(number(element.points[i].x)),\(number(element.points[i].y))" }
        switch element.type {
        case .moveToPoint: commands.append("M\(p(0))")
        case .addLineToPoint: commands.append("L\(p(0))")
        case .addQuadCurveToPoint: commands.append("Q\(p(0)) \(p(1))")
        case .addCurveToPoint: commands.append("C\(p(0)) \(p(1)) \(p(2))")
        case .closeSubpath: commands.append("Z")
        @unknown default: fatalError("Unexpected Core Graphics path element")
        }
    }
    output.append(commands.joined())
}
FileHandle.standardOutput.write(try JSONEncoder().encode(output))
