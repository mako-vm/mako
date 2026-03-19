class Mako < Formula
  desc "Fast, lightweight Docker Desktop alternative for macOS"
  homepage "https://github.com/mako-vm/mako"
  license "Apache-2.0"

  # Updated on each release
  url "https://github.com/mako-vm/mako/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "" # TODO: fill in on release
  version "0.1.0"

  depends_on :macos
  depends_on "rust" => :build
  depends_on "e2fsprogs"

  def install
    system "cargo", "build", "--release"
    system "cargo", "build", "--release",
           "--target", "aarch64-unknown-linux-musl",
           "-p", "mako-agent"

    bin.install "target/release/mako"
    bin.install "target/release/makod"

    # Install entitlements for codesigning
    (share/"mako").install "crates/daemon/entitlements.plist"
  end

  def post_install
    system "codesign", "--entitlements",
           "#{share}/mako/entitlements.plist",
           "--force", "-s", "-",
           "#{bin}/makod"
  end

  def caveats
    <<~EOS
      To get started:
        mako setup                    # Download VM image (first time)
        mako start                    # Start the VM
        export DOCKER_HOST=unix://$HOME/.mako/docker.sock
        docker ps                     # Use Docker as usual

      To enable launch at login, run the Mako GUI:
        cd gui/MakoApp && swift build -c release
        .build/release/MakoApp
    EOS
  end

  test do
    assert_match "mako", shell_output("#{bin}/mako --version")
  end
end
