#!/bin/sh
# Spike only: a box CA constrained to one name (option B), a leaf for the
# platform host plus its fragments, and a rogue leaf outside the constraint
# (which every client must refuse). Usage: mkca.sh <box fqdn> <out dir>
set -eu
BOX="$1"; OUT="$2"; mkdir -p "$OUT"; cd "$OUT"
openssl ecparam -name prime256v1 -genkey -noout -out ca.key
cat > ca.cnf <<CNF
[req]
distinguished_name=dn
prompt=no
x509_extensions=v3
[dn]
CN=Fragment box CA (spike) $BOX
[v3]
basicConstraints=critical,CA:TRUE,pathlen:0
keyUsage=critical,keyCertSign,cRLSign
subjectKeyIdentifier=hash
nameConstraints=critical,permitted;DNS:$BOX
CNF
openssl req -x509 -new -key ca.key -sha256 -days 825 -config ca.cnf -out ca.crt
openssl x509 -in ca.crt -outform DER -out ca.der
leaf() { # name, sans
  openssl ecparam -name prime256v1 -genkey -noout -out "$1.key"
  openssl req -new -key "$1.key" -subj "/CN=$2" -out "$1.csr"
  printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=%s\nauthorityKeyIdentifier=keyid\n' "$3" > "$1.ext"
  openssl x509 -req -in "$1.csr" -CA ca.crt -CAkey ca.key -CAcreateserial -days 90 -sha256 -extfile "$1.ext" -out "$1.leaf"
  cat "$1.leaf" ca.crt > "$1.crt"; rm -f "$1.csr" "$1.ext" "$1.leaf"
}
leaf box "$BOX" "DNS:$BOX,DNS:*.$BOX"
leaf rogue "rogue.example.org" "DNS:rogue.example.org,DNS:*.rogue.example.org"
openssl x509 -in ca.crt -noout -subject -ext nameConstraints
openssl verify -CAfile ca.crt box.crt
openssl verify -CAfile ca.crt rogue.crt || echo "rogue refused by openssl (expected)"
