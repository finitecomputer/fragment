#!/bin/bash
# Local models on the box's Mac (docs/self-host.md): builds whisper.cpp's
# server with Metal and an audio-only ffmpeg from their source releases, and
# fetches the transcription and VAD models, each pinned by sha256, into
# ~/local-models. Needs the Command Line Tools alone (no Xcode, no
# Homebrew): CMake is built from its source too when there is none.
# Runs nothing it downloads before checking it. Then install-mac.sh.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
D="${LOCAL_MODELS_DIR:-$HOME/local-models}"
mkdir -p "$D/src" "$D/bin" "$D/models" "$D/tools"
cd "$D/src"
jobs=$(sysctl -n hw.ncpu)

fetch() { # url sha256 file
  [ -f "$3" ] || curl -fL --retry 3 -o "$3.part" "$1"
  [ -f "$3" ] || mv "$3.part" "$3"
  echo "$2  $3" | shasum -a 256 -c -
}

# CMake 4.4.5 (2026-10-27), its source; Kitware's cmake-4.4.5-SHA-256.txt
fetch https://github.com/Kitware/CMake/releases/download/v4.4.5/cmake-4.4.5.tar.gz \
  a0c54c88abb8078dd8eccadb717fc6bb194bf18e7d44c86309910288886e04e2 cmake-4.4.5.tar.gz
# whisper.cpp v1.9.5 (2026-10-06; tag object d2a764c6, commit d1be6fde)
fetch https://github.com/ggml-org/whisper.cpp/archive/refs/tags/v1.9.5.tar.gz \
  ff1a9053feb509ff9d7729703355541ae9690073a6b1c40eb692c962e0dc1720 whisper.cpp-v1.9.5.tar.gz
# FFmpeg 9.0.2 (2026-09-18), signed by FFmpeg's release key
# FCF9 86EA 15E6 E293 A564 4F10 B432 2F04 D676 58D8
fetch https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz \
  8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e ffmpeg-9.0.2.tar.xz
# the models, as Hugging Face's LFS records them (and whisper.cpp's
# models/README.md, by SHA-1)
fetch https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin \
  394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2 "$D/models/ggml-large-v3-turbo-q5_0.bin"
fetch https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v6.2.0.bin \
  2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987 "$D/models/ggml-silero-v6.2.0.bin"

cmake=$(command -v cmake || true)
if [ -z "$cmake" ]; then
  if [ ! -x "$D/tools/cmake/bin/cmake" ]; then
    rm -rf cmake-4.4.5 && tar xzf cmake-4.4.5.tar.gz
    (cd cmake-4.4.5 && ./bootstrap --prefix="$D/tools/cmake" --parallel="$jobs" -- -DCMAKE_USE_OPENSSL=OFF -DBUILD_TESTING=OFF && make -j"$jobs" && make install)
  fi
  cmake="$D/tools/cmake/bin/cmake"
fi

# whisper.cpp's server: Metal, its shaders embedded and compiled at start
# (so no Xcode `metal` tool), static; patched (whisper-server.patch) so it
# serves no /load, which would let a caller swap the model, and leaves no
# upload behind when ffmpeg cannot read it
rm -rf whisper.cpp-1.9.5 && tar xzf whisper.cpp-v1.9.5.tar.gz
(cd whisper.cpp-1.9.5 \
  && patch -p1 < "$here/whisper-server.patch" \
  && "$cmake" -B build -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DGGML_METAL=ON -DGGML_METAL_EMBED_LIBRARY=ON \
       -DWHISPER_BUILD_TESTS=OFF -DWHISPER_BUILD_EXAMPLES=ON -DWHISPER_BUILD_SERVER=ON -DWHISPER_SDL2=OFF -DWHISPER_CURL=OFF \
  && "$cmake" --build build -j"$jobs" --target whisper-server)
install -m 0755 whisper.cpp-1.9.5/build/bin/whisper-server "$D/bin/whisper-server"

# ffmpeg for the server's --convert: audio alone, the containers and codecs
# browsers, phones and chat apps record (WebM/Ogg Opus, MP4/AAC, MP3, FLAC,
# WAV), WAV out; no network, no system libraries autodetected
rm -rf ffmpeg-9.0.2 && tar xJf ffmpeg-9.0.2.tar.xz
(cd ffmpeg-9.0.2 \
  && ./configure --prefix="$PWD/install" --cc=clang \
       --disable-everything --disable-autodetect --disable-network --disable-doc --disable-debug \
       --disable-programs --enable-ffmpeg --disable-avdevice --disable-swscale \
       --enable-protocol=file,pipe \
       --enable-demuxer=wav,w64,mp3,flac,ogg,matroska,mov,aac,caf,aiff \
       --enable-decoder=pcm_s16le,pcm_s16be,pcm_s24le,pcm_s32le,pcm_f32le,pcm_u8,pcm_alaw,pcm_mulaw,mp3,mp3float,flac,vorbis,opus,aac,aac_latm,alac \
       --enable-parser=aac,aac_latm,mpegaudio,flac,opus,vorbis \
       --enable-muxer=wav --enable-encoder=pcm_s16le \
       --enable-filter=aresample,aformat,anull,abuffer,abuffersink,amix,pan \
       --enable-swresample \
  && make -j"$jobs" && make install)
install -m 0755 ffmpeg-9.0.2/install/bin/ffmpeg "$D/bin/ffmpeg"

# what was built and fetched, pinned: whisper.sh checks it at every start
cd "$D"
shasum -a 256 bin/whisper-server bin/ffmpeg models/ggml-large-v3-turbo-q5_0.bin models/ggml-silero-v6.2.0.bin > SHA256SUMS
cat SHA256SUMS
