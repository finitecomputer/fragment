import SwiftUI

struct AppSharingView: View {
    let fragment: FragmentSummary
    let library: Library
    let api: FragmentAPI
    @State private var details: AppDetails?
    @State private var members: [AppMember] = []
    @State private var invites: [AppInvite] = []
    @FocusState private var emailFocused: Bool
    @State private var email = ""
    @State private var permission = AppPermission.viewer
    @State private var busy = false
    @State private var loading = true
    @State private var error: String?
    @State private var resetLink = false
    private var owner: Bool { details?.role == "owner" }
    private var visibleMembers: [AppMember] {
        members.filter { member in
            !(member.kind == "agent" && member.owner.map { id in members.contains { $0.principal == id } } == true)
        }
    }
    var body: some View {
        Form {
            if loading { ProgressView("Loading permissions…") }
            if let details {
                Section {
                    if owner {
                        Picker("General access", selection: Binding(
                            get: { AppAccess(rawValue: details.visibility) ?? .members },
                            set: { value in Task { await change { let _: JSONValue = try await api.request("api/f/\(fragment.name)/visibility", method: "PUT", body: .object(["visibility": .string(value.rawValue)])) } } }
                        )) { ForEach(AppAccess.allCases, id: \.self) { Text($0.title).tag($0) } }
                        .accessibilityIdentifier("appVisibility")
                    } else { LabeledContent("General access", value: AppAccess(rawValue: details.visibility)?.title ?? details.visibility) }
                } footer: { Text(AppAccess(rawValue: details.visibility)?.explanation ?? "") }
                if let url = details.shareURL {
                    Section("Share link") {
                        ShareLink(item: url) { Label("Share link", systemImage: "square.and.arrow.up") }
                        Button("Copy link", systemImage: "doc.on.doc") { UIPasteboard.general.url = url }
                        if owner && details.visibility == "link" { Button("Reset share link", systemImage: "arrow.clockwise") { resetLink = true } }
                    }
                }
            }
            Section("People with access") {
                ForEach(visibleMembers) { member in
                    HStack {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(memberTitle(member)).lineLimit(2)
                            if members.contains(where: { $0.kind == "agent" && $0.owner == member.principal }) {
                                Text("With their agents").font(.caption).foregroundStyle(.secondary)
                            }
                        }
                        Spacer()
                        if owner && member.role != "owner" {
                            Menu {
                                ForEach(AppPermission.allCases, id: \.self) { role in
                                    Button(role.title) { Task { await change { let _: JSONValue = try await api.request("api/f/\(fragment.name)/members/\(member.principal)", method: "PUT", body: .object(["role": .string(role.rawValue)])) } } }
                                }
                                Button("Remove access", role: .destructive) { Task { await change { let _: JSONValue = try await api.request("api/f/\(fragment.name)/members/\(member.principal)", method: "DELETE") } } }
                            } label: { Text(AppPermission(rawValue: member.role)?.title ?? member.role.capitalized).font(.subheadline) }
                            .accessibilityIdentifier("memberRole-\(member.principal)")
                        } else { Text(member.role == "owner" ? "Owner" : AppPermission(rawValue: member.role)?.title ?? member.role).font(.subheadline).foregroundStyle(.secondary) }
                    }.padding(.vertical, 4)
                }
            }
            if owner {
                Section("Invite someone") {
                    TextField("Email address", text: $email).keyboardType(.emailAddress).textContentType(.emailAddress)
                        .textInputAutocapitalization(.never).autocorrectionDisabled().focused($emailFocused).accessibilityIdentifier("inviteEmail")
                    Picker("Permission", selection: $permission) { ForEach(AppPermission.allCases, id: \.self) { Text($0.title).tag($0) } }
                    Button("Send invitation", systemImage: "envelope") { emailFocused = false; Task { await invite() } }
                        .disabled(email.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty).accessibilityIdentifier("sendAppInvite")
                }
                if !invites.isEmpty {
                    Section("Pending invitations") {
                        ForEach(invites) { invitation in
                            VStack(alignment: .leading, spacing: 5) {
                                Text(invitation.email)
                                Text(AppPermission(rawValue: invitation.role)?.title ?? invitation.role).font(.caption).foregroundStyle(.secondary)
                                Button("Revoke invitation", role: .destructive) { Task { await change { let _: JSONValue = try await api.request("api/f/\(fragment.name)/invites/\(invitation.email)", method: "DELETE") } } }
                                    .font(.caption).accessibilityIdentifier("revokeInvite-\(invitation.email)")
                            }.padding(.vertical, 4)
                        }
                    }
                }
            }
            if busy { ProgressView("Saving…") }
            if let error { Section { Text(error).font(.footnote).foregroundStyle(.secondary); Button("Reload permissions") { Task { await load() } } } }
        }
        .disabled(busy)
        .navigationTitle(owner ? "Sharing & Permissions" : "People with Access").navigationBarTitleDisplayMode(.inline)
        .task { await load() }
        .confirmationDialog("Reset this share link?", isPresented: $resetLink, titleVisibility: .visible) {
            Button("Reset share link", role: .destructive) { Task { await change { let _: JSONValue = try await api.request("api/f/\(fragment.name)/rotate", method: "POST", body: .object(["scopes": .array([.string("view")])])) } } }
        } message: { Text("The old link will stop working. People with membership keep their access.") }
    }
    private func memberTitle(_ member: AppMember) -> String {
        if member.principal == details?.owner { return owner ? "You" : "Owner" }
        return member.email ?? member.title ?? library.agent(for: member.principal)?.displayTitle ?? member.principal
    }
    private func load() async {
        loading = true; defer { loading = false }
        do {
            async let status = api.appDetails(fragment.name)
            async let people = api.appMembers(fragment.name)
            let (newDetails, newMembers) = try await (status, people)
            let newInvites = newDetails.role == "owner" ? try await api.appInvites(fragment.name) : []
            details = newDetails; members = newMembers; invites = newInvites; error = nil
        } catch { self.error = error.localizedDescription }
    }
    private func change(_ action: () async throws -> Void) async {
        busy = true; error = nil; defer { busy = false }
        do { try await action(); await load(); await library.load(api) }
        catch { self.error = error.localizedDescription }
    }
    private func invite() async {
        let clean = email.trimmingCharacters(in: .whitespacesAndNewlines)
        guard clean.count <= 254, clean.contains("@"), !clean.contains(where: { $0.isWhitespace || $0 == "/" || $0 == "?" || $0 == "#" || $0 == "%" }) else { error = "Enter a valid email address."; return }
        await change {
            let _: JSONValue = try await api.request("api/f/\(fragment.name)/invites", method: "POST", body: .object(["email": .string(clean), "role": .string(permission.rawValue)]))
            email = ""
        }
    }
}
