# env.sh: what every login shell on a fragment Boat sandbox needs. build-golden.sh
# copies it to ~/.boat-keep and sources it from ~/.profile (.bashrc returns
# early in a non-interactive shell, which is how `boat exec` and `boat run` run).
export CC_wasm32_unknown_unknown=clang AR_wasm32_unknown_unknown=llvm-ar
export CHROME_BIN=/usr/bin/google-chrome
# the Docker build cache does not survive a snapshot; a local cache folder does
export FRAGMENT_IMAGE_CACHE="type=local,src=$HOME/.buildcache,dest=$HOME/.buildcache"
