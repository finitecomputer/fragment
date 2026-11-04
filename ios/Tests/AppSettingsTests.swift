import Foundation
import ImageIO
import Testing
import UIKit
@testable import Fragment

/// An app's settings against a stub of the platform's answers: what the
/// sheet reads of status, members and invites, and what it sends.
@MainActor
struct AppSettingsTests {
    private func details(_ json: String) throws -> AppDetails { try JSONDecoder().decode(AppDetails.self, from: Data(json.utf8)) }

    @Test func theShareLinkIsTheOwnersOnALinkAppAndTheAddressOnAPublicOne() throws {
        let canonical = #""urls":{"canonical":"https://reading--p1.finite.test/","platform":"https://finite.test"}"#
        let link = try details(#"{"owner":"npub1me","role":"owner","visibility":"link","viewToken":"v1",\#(canonical)}"#)
        #expect(link.shareURL?.absoluteString == "https://reading--p1.finite.test/?view=v1" && link.canEdit)
        #expect(try details(#"{"owner":"npub1o","role":"editor","visibility":"link",\#(canonical)}"#).shareURL == nil)
        #expect(try details(#"{"owner":"npub1o","role":"viewer","visibility":"public",\#(canonical)}"#).shareURL?.absoluteString == "https://reading--p1.finite.test/")
        let members = try details(#"{"owner":"npub1me","role":"owner","visibility":"members","viewToken":"v1",\#(canonical)}"#)
        #expect(members.shareURL == nil)
        #expect(try details(#"{"owner":"npub1o","role":"viewer","visibility":"link",\#(canonical)}"#).canEdit == false)
    }

    @Test func membersAndInvitesAreReadAsListed() async throws {
        let stub = Stub { seen, _ in
            switch seen.route {
            case "GET /api/f/reading--p1/members": (200, #"{"members":[{"principal":"npub1me","role":"owner","addedBy":"npub1me","addedAt":1,"kind":"person"},{"principal":"npub1moss","role":"editor","addedBy":"npub1me","addedAt":2,"kind":"agent","owner":"npub1me"}]}"#)
            case "GET /api/f/reading--p1/invites": (200, #"{"invites":[{"email":"friend@example.com","role":"viewer","expiresAt":9,"createdBy":"npub1me"}]}"#)
            default: (404, "{}")
            }
        }
        let api = try stub.api()
        let members = try await api.appMembers("reading--p1")
        #expect(members.map(\.principal) == ["npub1me", "npub1moss"] && members[1].owner == "npub1me")
        #expect(try await api.appInvites("reading--p1").map(\.email) == ["friend@example.com"])
    }

    @Test func iconsAreMadeWithAHintAndARefusalSaysWhy() async throws {
        let stub = Stub { seen, _ in
            switch seen.route {
            case "POST /api/f/reading--p1/icon/make": (200, #"{"subject":"a book","candidates":[{"png":"iVBORw0KGgo=","score":0.9}],"flaws":{}}"#)
            case "PUT /api/f/reading--p1/icon": (400, #"{"error":"invalid_request","message":"an icon is a PNG, JPEG, WebP or GIF"}"#)
            case "GET /api/f/reading--p1/icon": (404, #"{"error":"not_found","message":"no icon"}"#)
            default: (404, "{}")
            }
        }
        let api = try stub.api()
        let options = try await api.makeAppIcons("reading--p1", hint: "  a book ")
        #expect(options.candidates.first?.data != nil)
        #expect(stub.seen[0].json == .object(["hint": .string("a book")]))
        _ = try await api.makeAppIcons("reading--p1", hint: " ")
        #expect(stub.seen[1].json == .object([:]))
        do {
            try await api.putAppIcon(Data("not an image".utf8), name: "reading--p1")
            Issue.record("a refused icon is an error")
        } catch APIError.refused(let message) { #expect(message == "an icon is a PNG, JPEG, WebP or GIF") }
        #expect(try await api.appImage("reading--p1", kind: "icon") == nil)
    }

    @Test func aPickedPhotoIsFittedOnThePhoneBeforeItIsSent() throws {
        let file = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString + ".png")
        defer { try? FileManager.default.removeItem(at: file) }
        let large = UIGraphicsImageRenderer(size: CGSize(width: 2400, height: 1200), format: { let f = UIGraphicsImageRendererFormat(); f.scale = 1; return f }()).image { context in
            UIColor.systemTeal.setFill(); context.fill(CGRect(x: 0, y: 0, width: 2400, height: 1200))
        }
        try #require(large.pngData()).write(to: file)
        let fitted = try AppIconImage.fitted(file)
        let source = try #require(CGImageSourceCreateWithData(fitted as CFData, nil))
        let properties = try #require(CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any])
        #expect(properties[kCGImagePropertyPixelWidth] as? Int == 256 && properties[kCGImagePropertyPixelHeight] as? Int == 128)
        #expect(CGImageSourceGetType(source) as String? == "public.png")
        let text = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString + ".txt")
        defer { try? FileManager.default.removeItem(at: text) }
        try Data("not an image".utf8).write(to: text)
        #expect(throws: APIError.self) { try AppIconImage.fitted(text) }
    }
}
