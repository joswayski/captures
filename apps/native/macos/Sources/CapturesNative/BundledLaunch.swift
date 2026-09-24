import AppKit

/// Receives the initial LaunchServices document event before creating any host UI.
final class BundledLaunch: NSObject, NSApplicationDelegate {
    private var options: Options
    private var receivedFiles = false
    private let start: (Options, Notification) throws -> Bool
    private let reply: (NSApplication.DelegateReply) -> Void
    private let finish: (Int32) -> Void

    init(options: Options,
         reply: @escaping (NSApplication.DelegateReply) -> Void = { NSApp.reply(toOpenOrPrint: $0) },
         finish: @escaping (Int32) -> Void = { exit($0) },
         start: @escaping (Options, Notification) throws -> Bool) {
        self.options = options
        self.reply = reply
        self.finish = finish
        self.start = start
    }

    func application(_ sender: NSApplication, openFiles filenames: [String]) {
        options.openMedia.append(contentsOf: filenames)
        receivedFiles = true
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        do {
            let primary = try start(options, notification)
            if receivedFiles { reply(.success) }
            if !primary { finish(0) }
        } catch {
            if receivedFiles { reply(.failure) }
            FileHandle.standardError.write(Data("Captures could not open files: \(error.localizedDescription)\n".utf8))
            finish(1)
        }
    }
}

enum NativeResources {
    static let bundle: Bundle = {
        // SwiftPM's generated accessor can fall back to an absolute build path.
        // A packaged app must load its own resources, never the build checkout.
        guard Bundle.main.object(forInfoDictionaryKey: "CapturesNativeLive") as? Bool == true
        else { return Bundle.module }
        guard let url = Bundle.main.url(forResource: "CapturesNative_CapturesNative", withExtension: "bundle"),
              let bundle = Bundle(url: url)
        else { fatalError("Captures Native is missing its bundled resources") }
        return bundle
    }()
}
