class Shuffle < Formula
  desc "Command-line MP3 player with shuffle playback and arrow-key controls"
  homepage "https://github.com/ddnn55/shuffle"
  url "file://#{File.expand_path("..", __dir__)}"
  version "0.1.0"
  sha256 :no_check

  depends_on "rust" => :build
  depends_on :macos

  def install
    system "cargo", "build", "--release", "--bin", "shuffle"
    bin.install "target/release/shuffle"
  end

  test do
    assert_match(
      "No mp3 files found.",
      shell_output("#{bin}/shuffle 2>&1", 1)
    )
  end
end
