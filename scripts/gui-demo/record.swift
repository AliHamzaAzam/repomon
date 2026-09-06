// Capture only the verified demo window at native Retina resolution.
import Cocoa
import ScreenCaptureKit
import AVFoundation
import VideoToolbox
import ImageIO
import UniformTypeIdentifiers

final class Recording: NSObject, SCStreamOutput, SCStreamDelegate {
    let directory: URL
    let ready: URL
    let queue = DispatchQueue(label: "demo.lossless.frames")
    private let lock = NSLock()
    private var failure: Error?
    private var frames: [(String, Double)] = []
    init(directory: URL, ready: URL) { self.directory = directory; self.ready = ready }
    func stream(_ stream: SCStream, didStopWithError error: Error) {
        lock.lock(); failure = error; lock.unlock()
    }
    func check() throws {
        lock.lock(); defer { lock.unlock() }
        if let failure { throw failure }
    }
    func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, sample.isValid,
              let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
              let status = attachments.first?[.status] as? Int,
              status == SCFrameStatus.complete.rawValue,
              let buffer = sample.imageBuffer else { return }
        do {
            var image: CGImage?
            guard VTCreateCGImageFromCVPixelBuffer(buffer, options: nil, imageOut: &image) == noErr, let image else {
                throw NSError(domain: "DemoCapture", code: 2, userInfo: [NSLocalizedDescriptionKey: "Frame conversion failed"])
            }
            let name = String(format: "frame-%06d.png", frames.count)
            try savePNG(image, to: directory.appendingPathComponent(name))
            frames.append((name, sample.presentationTimeStamp.seconds))
            if frames.count == 1 { try Data("ready".utf8).write(to: ready, options: .atomic) }
        } catch {
            lock.lock(); failure = error; lock.unlock()
        }
    }
    func finish() throws {
        queue.sync {}
        try check()
        guard !frames.isEmpty else { throw NSError(domain: "DemoCapture", code: 3) }
        var manifest = "ffconcat version 1.0\n"
        for (index, frame) in frames.enumerated() {
            let duration = index + 1 < frames.count ? frames[index + 1].1 - frame.1 : 1.0 / 12
            manifest += "file '\(frame.0)'\nduration \(max(duration, 0.001))\n"
        }
        manifest += "file '\(frames.last!.0)'\n"
        try manifest.write(to: directory.appendingPathComponent("frames.ffconcat"), atomically: true, encoding: .utf8)
    }
}
func savePNG(_ image: CGImage, to url: URL) throws {
    guard let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        throw NSError(domain: "DemoCapture", code: 4)
    }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else { throw NSError(domain: "DemoCapture", code: 5) }
}
let args = CommandLine.arguments
guard (args.count == 4 || args.count == 6), let pid = Int32(args[1]), let id = UInt32(args[2]) else {
    fputs("usage: record.swift PID WINDOW_ID PNG_OR_FRAME_DIRECTORY [STOP_FILE READY_FILE]\n", stderr); exit(1)
}
let output = URL(fileURLWithPath: args[3])
let still = args.count == 4
let stop = still ? output : URL(fileURLWithPath: args[4])
let ready = still ? output : URL(fileURLWithPath: args[5])
guard output.path.hasPrefix("/private/tmp/repomon-gui-demo."),
      stop.deletingLastPathComponent() == output.deletingLastPathComponent(),
      ready.deletingLastPathComponent() == output.deletingLastPathComponent() else {
    fputs("Output must remain in the demo sandbox\n", stderr); exit(1)
}
_ = NSApplication.shared
_ = CGMainDisplayID()
Task { @MainActor in
    do {
        let content = try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: true)
        guard let window = content.windows.first(where: { $0.windowID == id && $0.owningApplication?.processID == pid }) else {
            throw NSError(domain: "DemoCapture", code: 1, userInfo: [NSLocalizedDescriptionKey: "PID-owned window missing"])
        }
        let filter = SCContentFilter(desktopIndependentWindow: window)
        let config = SCStreamConfiguration()
        config.width = Int((filter.contentRect.width * CGFloat(filter.pointPixelScale)).rounded())
        config.height = Int((filter.contentRect.height * CGFloat(filter.pointPixelScale)).rounded())
        config.pixelFormat = kCVPixelFormatType_32BGRA
        config.minimumFrameInterval = CMTime(value: 1, timescale: 12)
        config.queueDepth = 8
        config.showsCursor = false
        config.capturesAudio = false
        config.captureMicrophone = false
        config.ignoreShadowsSingleWindow = true
        config.includeChildWindows = false
        if still {
            let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
            try savePNG(image, to: output)
            exit(0)
        }
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: false)
        let delegate = Recording(directory: output, ready: ready)
        let stream = SCStream(filter: filter, configuration: config, delegate: delegate)
        try stream.addStreamOutput(delegate, type: .screen, sampleHandlerQueue: delegate.queue)
        try await stream.startCapture()
        let deadline = Date().addingTimeInterval(600)
        while !FileManager.default.fileExists(atPath: stop.path) {
            try delegate.check()
            if Date() > deadline { throw NSError(domain: "DemoCapture", code: 6, userInfo: [NSLocalizedDescriptionKey: "Tour timed out"] ) }
            try await Task.sleep(nanoseconds: 200_000_000)
        }
        try await stream.stopCapture()
        try delegate.finish()
        exit(0)
    } catch {
        fputs("Demo capture failed: \(error)\n", stderr)
        exit(1)
    }
}
RunLoop.main.run()
