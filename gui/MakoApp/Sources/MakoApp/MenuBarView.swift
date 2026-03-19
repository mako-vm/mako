import SwiftUI

struct MenuBarView: View {
    @ObservedObject var viewModel: MakoViewModel

    var body: some View {
        VStack(spacing: 0) {
            headerSection
            Divider()
            if viewModel.isRunning {
                containersList
                Divider()
                infoSection
            } else {
                stoppedView
            }
            Divider()
            footerSection
        }
        .frame(width: 340)
    }

    // MARK: - Header

    private var headerSection: some View {
        HStack {
            Image(systemName: "cube.fill")
                .font(.title2)
                .foregroundColor(viewModel.isRunning ? .green : .secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text("Mako")
                    .font(.headline)
                if let msg = viewModel.statusMessage {
                    Text(msg)
                        .font(.caption)
                        .foregroundColor(.orange)
                } else {
                    Text(viewModel.isRunning ? "Running" : "Stopped")
                        .font(.caption)
                        .foregroundColor(viewModel.isRunning ? .green : .secondary)
                }
            }
            Spacer()
            Button(action: {
                if viewModel.isRunning { viewModel.stopMako() }
                else { viewModel.startMako() }
            }) {
                Image(systemName: viewModel.isRunning ? "stop.fill" : "play.fill")
                    .font(.title3)
            }
            .buttonStyle(.borderless)
            .help(viewModel.isRunning ? "Stop Mako" : "Start Mako")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }

    // MARK: - Containers

    private var containersList: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Containers")
                    .font(.caption)
                    .foregroundColor(.secondary)
                    .textCase(.uppercase)
                Spacer()
                Text("\(viewModel.containers.filter(\.isRunning).count) running")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
            .padding(.horizontal, 16)
            .padding(.top, 10)
            .padding(.bottom, 6)

            if viewModel.containers.isEmpty {
                Text("No containers")
                    .font(.caption)
                    .foregroundColor(.secondary)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 20)
            } else {
                ScrollView {
                    LazyVStack(spacing: 1) {
                        ForEach(viewModel.containers) { container in
                            ContainerRow(container: container, viewModel: viewModel)
                        }
                    }
                }
                .frame(maxHeight: 220)
            }
        }
    }

    // MARK: - Info

    private var infoSection: some View {
        VStack(spacing: 6) {
            HStack {
                Label("Docker", systemImage: "shippingbox")
                    .font(.caption)
                    .foregroundColor(.secondary)
                Spacer()
                Text(viewModel.dockerVersion)
                    .font(.caption)
                    .foregroundColor(.primary)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
    }

    // MARK: - Stopped

    private var stoppedView: some View {
        VStack(spacing: 12) {
            Image(systemName: "cube")
                .font(.system(size: 36))
                .foregroundColor(.secondary)
            if let msg = viewModel.statusMessage {
                Text(msg)
                    .font(.caption)
                    .foregroundColor(.orange)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 12)
            } else {
                Text("Mako is not running")
                    .font(.subheadline)
                    .foregroundColor(.secondary)
            }
            Button("Start Mako") {
                viewModel.startMako()
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.regular)
            .disabled(viewModel.statusMessage != nil)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 40)
    }

    // MARK: - Footer

    private var footerSection: some View {
        HStack {
            Button("Quit") {
                NSApplication.shared.terminate(nil)
            }
            .buttonStyle(.borderless)
            .font(.caption)
            .foregroundColor(.secondary)
            Spacer()
            Text("v0.1.0")
                .font(.caption2)
                .foregroundColor(.secondary.opacity(0.5))
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
    }
}

struct ContainerRow: View {
    let container: ContainerInfo
    @ObservedObject var viewModel: MakoViewModel

    var body: some View {
        HStack(spacing: 10) {
            Circle()
                .fill(container.isRunning ? Color.green : Color.secondary.opacity(0.3))
                .frame(width: 8, height: 8)

            VStack(alignment: .leading, spacing: 1) {
                Text(container.displayName)
                    .font(.system(.body, design: .monospaced))
                    .lineLimit(1)
                Text(container.Image)
                    .font(.caption2)
                    .foregroundColor(.secondary)
                    .lineLimit(1)
            }

            Spacer()

            Text(container.Status)
                .font(.caption2)
                .foregroundColor(.secondary)
                .lineLimit(1)

            Menu {
                if container.isRunning {
                    Button("Stop") { viewModel.stopContainer(container.id) }
                } else {
                    Button("Start") { viewModel.startContainer(container.id) }
                }
                Divider()
                Button("Remove", role: .destructive) { viewModel.removeContainer(container.id) }
            } label: {
                Image(systemName: "ellipsis.circle")
                    .font(.caption)
            }
            .menuStyle(.borderlessButton)
            .frame(width: 20)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
        .background(Color.primary.opacity(0.02))
    }
}
