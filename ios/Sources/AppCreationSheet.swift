import SwiftUI

// Like the web shell's Add hand, this starts an agent conversation; it is
// not an app named "add-app" on the server.
struct AppCreationSheet: View {
    let start: (String) -> Void
    @State private var idea = ""
    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text("Ask for an app").font(.title2.weight(.semibold))
                    Text("Tell your agent who it’s for and what it should do.").foregroundStyle(.secondary)
                    TextField("Describe your app…", text: $idea, axis: .vertical).lineLimit(3...8)
                    Button("Start a chat", systemImage: "arrow.up") { start(idea) }
                        .disabled(idea.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || idea.utf8.count > 32_768)
                }
                Section("A few ideas") {
                    Button("A grocery list for the family") { start("Make a shared grocery list my family can use") }
                    Button("A reading list worth returning to") { start("Make a reading list for things I want to come back to") }
                    Button("A little morning briefing") { start("Every morning send me a summary of the sites I follow") }
                }
            }
            .navigationTitle("Add an app").navigationBarTitleDisplayMode(.inline)
        }
        .presentationDetents([.large]).presentationDragIndicator(.visible)
    }
}
