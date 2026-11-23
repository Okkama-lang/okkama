#!/bin/sh
# NameConstraints over URI and an unsupported name form (#1014). EC P-256,
# valid 100 years; the tests check at 2030-01-01.
#
#   root.der     anchor "Okkama NC Root"
#   inter.der    CA under root: permitted DNS:nc.invalid and URI:.nc.invalid,
#                permitted otherName 1.2.3.4
#   uri-in.der   leaf a.nc.invalid, URI https://www.nc.invalid/x — inside
#   uri-out.der  leaf a.nc.invalid, URI https://evil.example/ — outside
#   other.der    leaf a.nc.invalid with an otherName (not interpreted)
set -eu
cd "$(dirname "$0")"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
key() { openssl ecparam -genkey -name prime256v1 -noout -out "$work/$1.key"; }
key root
openssl req -new -x509 -key "$work/root.key" -subj "/CN=Okkama NC Root" -days 36500 \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" \
  -addext "subjectKeyIdentifier=hash" -out "$work/root.pem"
sign() { # name subject issuer extensions
  key "$1"
  openssl req -new -key "$work/$1.key" -subj "$2" -out "$work/$1.csr"
  printf '%s\n' "$4" > "$work/$1.ext"
  openssl x509 -req -in "$work/$1.csr" -CA "$work/$3.pem" -CAkey "$work/$3.key" \
    -set_serial "0x$(openssl rand -hex 8)" -days 36500 -extfile "$work/$1.ext" -out "$work/$1.pem" 2>/dev/null
}
sign inter "/CN=Okkama NC Constrained" root "basicConstraints=critical,CA:TRUE,pathlen:0
keyUsage=critical,keyCertSign,cRLSign
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid
nameConstraints=critical,permitted;DNS:nc.invalid,permitted;URI:.nc.invalid,permitted;otherName:1.2.3.4;UTF8:allowed"
leaf() { sign "$1" "/CN=$1" inter "keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
subjectAltName=$2
authorityKeyIdentifier=keyid"; }
leaf uri-in "DNS:a.nc.invalid,URI:https://www.nc.invalid/x"
leaf uri-out "DNS:a.nc.invalid,URI:https://evil.example/"
leaf other "DNS:a.nc.invalid,otherName:1.2.3.4;UTF8:allowed"
for n in root inter uri-in uri-out other; do openssl x509 -in "$work/$n.pem" -outform DER -out "$n.der"; done
