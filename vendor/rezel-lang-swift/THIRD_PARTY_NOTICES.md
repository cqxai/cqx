# Third-party notices

## Swift compiler

Versioned source acceptance is compared with Swift compiler commit
`064859e41d68596f486c5d724401cb370f260409`, released as
`swift-6.3.3-RELEASE`. The matching Apple Swift 6.3.3 frontend is a
development-time oracle and is not linked into this crate.

Swift is licensed under Apache License 2.0 with Runtime Library Exception:
<https://github.com/swiftlang/swift/blob/swift-6.3.3-RELEASE/LICENSE.txt>.

## SwiftSyntax

Concrete-syntax structure, parser dispatch, newline separation, and official
parser cases are compared with SwiftSyntax commit
`60e8eb850721b5a6eebbd973b39f450a16553bd9`. Short inputs retained in
`tests/upstream.rs` identify their original `SwiftParserTest` locations.
SwiftSyntax is a development-time reference and is not linked into this crate.

SwiftSyntax is licensed under Apache License 2.0 with Runtime Library
Exception:
<https://github.com/swiftlang/swift-syntax/blob/60e8eb850721b5a6eebbd973b39f450a16553bd9/LICENSE.txt>.
