import CoreFoundation
import Foundation

guard CommandLine.arguments.count == 2, let command = Int32(CommandLine.arguments[1]) else {
    fputs("usage: media_remote_send.swift <command-number>\n", stderr)
    exit(2)
}

let bundleURL = URL(fileURLWithPath: "/System/Library/PrivateFrameworks/MediaRemote.framework")
guard let bundle = CFBundleCreate(kCFAllocatorDefault, bundleURL as CFURL) else {
    fputs("failed to load MediaRemote bundle\n", stderr)
    exit(1)
}

typealias SendCommand = @convention(c) (Int32, CFDictionary?) -> Bool
guard let symbol = CFBundleGetFunctionPointerForName(bundle, "MRMediaRemoteSendCommand" as CFString) else {
    fputs("failed to resolve MRMediaRemoteSendCommand\n", stderr)
    exit(1)
}

let sendCommand = unsafeBitCast(symbol, to: SendCommand.self)
let ok = sendCommand(command, nil)
print(ok ? "sent" : "failed")
