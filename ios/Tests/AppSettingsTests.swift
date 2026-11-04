import Foundation
import UIKit
import Testing
@testable import Fragment

@MainActor
struct AppSettingsTests {
    private let api = FragmentAPI(baseURL: URL(string: "http://localhost:8910")!)
    private func newApp() async throws -> FragmentSummary {
        let created: JSONValue = try await api.request("api/fragments", method: "POST", body: .object(["label": .string("settings-test"), "template": .string("blank"), "title": .string("App settings test")]))
        let name = try #require(created["name"].string)
        return try #require(try await api.fragments().first { $0.name == name })
    }
    private func refuses(_ action: () async throws -> Void) async {
        do { try await action(); Issue.record("Invalid or unauthorized mutation was accepted") }
        catch APIError.server { }
        catch { Issue.record("Unexpected failure: \(error)") }
    }
    @Test func hidingIsPersonalAndHiddenAppsRemainManageable() async throws {
        let app = try await newApp()
        let access = try await api.appDetails(app.name)
        for _ in 0..<2 { let _: JSONValue = try await api.request("api/fragments/\(app.name)/dock", method: "PUT", body: .object(["dock": .bool(false)])) }
        let library = Library(); await library.load(api)
        #expect(library.allApps.contains { $0.name == app.name && $0.dock == false })
        #expect(!library.shownApps.contains { $0.name == app.name })
        #expect(try await api.appDetails(app.name).visibility == access.visibility)
        await refuses { let _: JSONValue = try await api.request("api/fragments/\(app.name)/dock", method: "PUT", body: .object(["dock": .string("no")])) }
        #expect(try await api.fragments().first { $0.name == app.name }?.dock == false)
        let _: JSONValue = try await api.request("api/fragments/\(app.name)/dock", method: "PUT", body: .object(["dock": .bool(true)]))
        await library.load(api)
        #expect(library.shownApps.contains { $0.name == app.name })
    }
    @Test func sharingVisibilityAndLinkRotationKeepMembership() async throws {
        let app = try await newApp()
        let members = try await api.appMembers(app.name)
        for _ in 0..<2 { let _: JSONValue = try await api.request("api/f/\(app.name)/visibility", method: "PUT", body: .object(["visibility": .string("members")])) }
        #expect(try await api.appDetails(app.name).shareURL == nil)
        await refuses { let _: JSONValue = try await api.request("api/f/\(app.name)/visibility", method: "PUT", body: .object(["visibility": .string("everyone")])) }
        #expect(try await api.appDetails(app.name).visibility == "members")
        let _: JSONValue = try await api.request("api/f/\(app.name)/visibility", method: "PUT", body: .object(["visibility": .string("link")]))
        let before = try await api.appDetails(app.name)
        #expect(before.shareURL != nil)
        let _: JSONValue = try await api.request("api/f/\(app.name)/rotate", method: "POST", body: .object(["scopes": .array([.string("view")])]))
        let after = try await api.appDetails(app.name)
        #expect(before.shareURL != after.shareURL)
        #expect(try await api.appMembers(app.name).map(\.principal) == members.map(\.principal))
    }
    @Test func invitationRenewalRoleValidationAndRevocation() async throws {
        let app = try await newApp()
        for _ in 0..<2 { let _: JSONValue = try await api.request("api/f/\(app.name)/invites", method: "POST", body: .object(["email": .string("invite-test@finite.local"), "role": .string("viewer")])) }
        #expect(try await api.appInvites(app.name).count == 1)
        let _: JSONValue = try await api.request("api/f/\(app.name)/invites", method: "POST", body: .object(["email": .string("invite-test@finite.local"), "role": .string("editor")]))
        #expect(try await api.appInvites(app.name).first?.role == "editor")
        await refuses { let _: JSONValue = try await api.request("api/f/\(app.name)/invites", method: "POST", body: .object(["email": .string("invite-test@finite.local"), "role": .string("owner")])) }
        #expect(try await api.appInvites(app.name).first?.role == "editor")
        let _: JSONValue = try await api.request("api/f/\(app.name)/invites/invite-test@finite.local", method: "DELETE")
        #expect(try await api.appInvites(app.name).isEmpty)
        await refuses { let _: JSONValue = try await api.request("api/f/\(app.name)/invites/invite-test@finite.local", method: "DELETE") }
    }
    @Test func memberRolesAndRemovalRespectOwnership() async throws {
        let app = try await newApp()
        let agents = try await api.fragments()
        let moss = try #require(agents.first { $0.name == "moss" })
        try await api.join(moss, to: app); try await api.join(moss, to: app)
        #expect(try await api.appMembers(app.name).filter { $0.principal == "npub1localmoss" }.count == 1)
        await refuses { let _: JSONValue = try await api.request("api/f/\(app.name)/members/npub1localperson", method: "PUT", body: .object(["role": .string("viewer")])) }
        #expect(try await api.appMembers(app.name).first { $0.principal == "npub1localperson" }?.role == "owner")
        for _ in 0..<2 { let _: JSONValue = try await api.request("api/f/\(app.name)/members/npub1localmoss", method: "DELETE") }
        #expect(try await api.appMembers(app.name).allSatisfy { $0.principal != "npub1localmoss" })
    }
    @Test func viewerCannotWriteSharingIconsOrPreviews() async throws {
        let name = "shared-reading"
        #expect(try await api.appDetails(name).canEdit == false)
        await refuses { let _: JSONValue = try await api.request("api/f/\(name)/visibility", method: "PUT", body: .object(["visibility": .string("public")])) }
        await refuses { _ = try await api.makeAppIcons(name, hint: "") }
        await refuses { let _: JSONValue = try await api.request("api/f/\(name)/card", method: "POST") }
        await refuses { let _: JSONValue = try await api.request("api/f/\(name)/invites", method: "POST", body: .object(["email": .string("no@finite.local"), "role": .string("viewer")])) }
        #expect(try await api.appDetails(name).visibility == "link")
    }
    @Test func generatedIconMustBeChosenAndUploadsAreValidated() async throws {
        let app = try await newApp()
        let options = try await api.makeAppIcons(app.name, hint: "A notebook")
        let data = try #require(options.candidates.first?.data)
        #expect(try await api.appImage(app.name, kind: "icon") == nil)
        try await api.putAppIcon(data, name: app.name)
        let first = try #require(try await api.appImage(app.name, kind: "icon"))
        #expect(UIImage(data: first)?.size == CGSize(width: 256, height: 256))
        try await api.putAppIcon(data, name: app.name)
        #expect(try await api.appImage(app.name, kind: "icon") == first)
        await refuses { try await api.putAppIcon(Data("not an image".utf8), name: app.name) }
        #expect(try await api.appImage(app.name, kind: "icon") == first)
        await refuses { _ = try await api.makeAppIcons(app.name, hint: String(repeating: "x", count: 201)) }
        for _ in 0..<2 { let _: JSONValue = try await api.request("api/f/\(app.name)/icon", method: "DELETE") }
        #expect(try await api.appImage(app.name, kind: "icon") == nil)
    }
    @Test func previewWaitsForReportAndRestrictedAppsRefuseCapture() async throws {
        let app = try await newApp()
        let request: JSONValue = try await api.request("api/f/\(app.name)/card", method: "POST")
        let joined: JSONValue = try await api.request("api/f/\(app.name)/card", method: "POST")
        #expect(joined["joined"] == .bool(true))
        var details = try await api.appDetails(app.name)
        for _ in 0..<30 where details.page == nil {
            try await Task.sleep(for: .milliseconds(100)); details = try await api.appDetails(app.name)
        }
        #expect(details.page != nil)
        #expect(request["since"] != .null)
        let image = try #require(try await api.appImage(app.name, kind: "card"))
        #expect(UIImage(data: image)?.size == CGSize(width: 1280, height: 800))
        let _: JSONValue = try await api.request("api/f/\(app.name)/visibility", method: "PUT", body: .object(["visibility": .string("members")]))
        await refuses { let _: JSONValue = try await api.request("api/f/\(app.name)/card", method: "POST") }
    }
}
