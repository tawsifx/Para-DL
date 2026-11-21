require "json"

class ParallelDl < Formula
  desc "High-Performance Multi-Channel Download Accelerator with Anti-Throttle Watchdog"
  homepage "https://github.com/tawsifx/Para-DL"
  url "https://github.com/tawsifx/Para-DL/archive/refs/tags/v1.0.0.tar.gz"
  license "MIT"
  head "https://github.com/tawsifx/Para-DL.git", branch: "main"

  # The native Rust binary owns BOTH `paradl` and `parallel-dl`. Python is only
  # needed for the OPTIONAL media worker (python -m paradl_media), which lives
  # in a venv under libexec; there is no Python console script, no default
  # aria2 dependency, and no Python on any non-media path.
  depends_on "rust" => :build
  depends_on "python@3.12" => :recommended # media routes only
  depends_on "ffmpeg" => :recommended       # media routes only

  # Optional JavaScript runtime for YouTube challenge solving.
  depends_on "deno" => :recommended

  def python
    Formula["python@3.12"].opt_bin/"python3.12"
  end

  def media_venv
    libexec/"media-venv"
  end

  def install
    # One native binary owns both command names.
    system "cargo", "build", "--release", "--manifest-path", "rust/Cargo.toml", "--bin", "paradl"
    bin.install "rust/target/release/paradl" => "paradl"
    bin.install_symlink bin/"paradl" => "parallel-dl" unless (bin/"parallel-dl").exist?

    # OPTIONAL media venv: only media routes need Python. Installations built
    # without python@3.12 skip this block; point PARADL_MEDIA_PY at any external
    # venv to enable media routes later. yt-dlp is pip-installed into the venv,
    # never frozen into the binary (parity row P12), and the worker is invoked
    # as `python -m paradl_media`.
    worker_python = media_venv/"bin/python"
    if python.exist?
      system python, "-m", "venv", media_venv.to_s
      system worker_python, "-m", "pip", "install", "--upgrade", "pip"
      system worker_python, "-m", "pip", "install", "yt-dlp[default]", "mutagen"
      if (buildpath/"media/pyproject.toml").exist?
        system worker_python, "-m", "pip", "install", "./media"
      end

      (libexec/"media-python").write "#{worker_python}\n"
    else
      opoo "python@3.12 disabled: media routes need an external venv via PARADL_MEDIA_PY"
    end
  end

  def caveats
    <<~EOS
      `paradl` and `parallel-dl` are the same native binary. No Python is
      required for non-media routes.

      Media routes (yt-dlp / audio extraction) optionally use a Python worker:
        #{media_venv}
      Set PARADL_MEDIA_PY to override that interpreter. If the media venv was
      skipped, install Python 3.11+ with yt-dlp[default] + mutagen and export
      PARADL_MEDIA_PY to enable media routes.
    EOS
  end

  test do
    assert_match "paradl", shell_output("#{bin}/paradl --version")
    assert_match "parallel-dl", shell_output("#{bin}/parallel-dl --version")
    assert_match "ready", shell_output("#{bin}/paradl doctor")

    # The media worker is optional: --version and doctor above stay mandatory
    # and must pass with NO Python. The handshake is exercised only when the
    # media venv is actually present.
    worker = (media_venv/"bin/python").to_s
    if system(worker, "-c", "import paradl_media", out: File::NULL, err: File::NULL)
      request = {
        "protocol" => "paradl-media/1",
        "request_id" => "00000000-0000-4000-8000-000000000001",
        "op" => "claims",
        "target" => "https://example.com/video",
        "out_dir" => testpath.to_s,
        "options" => {},
      }
      output = shell_output("printf '%s\\n' '#{request.to_json}' | #{worker} -m paradl_media")
      frame = JSON.parse(output.lines.last)
      assert_equal "paradl-media/1", frame["protocol"]
      assert_equal "result", frame["frame"]
      assert_kind_of Hash, frame["versions"]
    else
      puts "media worker not installed; media handshake skipped (optional)"
    end
  end
end
