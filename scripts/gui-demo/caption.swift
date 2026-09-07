// Render burned-in captions with the site's Space Grotesk font. No GUI window is opened.
import Cocoa
import CoreText
let args = CommandLine.arguments
guard args.count == 4 else { fatalError("usage: caption.swift FONT OUTPUT TEXT") }
let output = URL(fileURLWithPath: args[2])
guard output.path.hasPrefix("/private/tmp/repomon-gui-demo.") else { fatalError("Sandbox output required") }
let fontURL = URL(fileURLWithPath: args[1])
CTFontManagerRegisterFontsForURL(fontURL as CFURL, .process, nil)
guard let font = NSFont(name: "SpaceGrotesk-Regular", size: 32) ?? NSFont(name: "SpaceGrotesk", size: 32) else { fatalError("Space Grotesk did not load") }
let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 1200, pixelsHigh: 96,
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
NSColor(calibratedWhite: 0.06, alpha: 1).setFill()
NSRect(x: 0, y: 0, width: 1200, height: 96).fill()
let style = NSMutableParagraphStyle(); style.alignment = .center
let caption = NSAttributedString(string: args[3], attributes: [.font: font, .foregroundColor: NSColor.white, .paragraphStyle: style])
let bounds = caption.boundingRect(with: NSSize(width: 1140, height: 96), options: [.usesLineFragmentOrigin])
guard bounds.height <= 84 else { fatalError("Caption exceeds two lines") }
caption.draw(in: NSRect(x: 30, y: (96 - bounds.height) / 2, width: 1140, height: bounds.height))
NSGraphicsContext.restoreGraphicsState()
try bitmap.representation(using: .png, properties: [:])!.write(to: output)
