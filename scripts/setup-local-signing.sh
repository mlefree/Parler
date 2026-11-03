#!/bin/bash
# One persistent, local-only code-signing identity. No Apple account is needed.
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
  echo "Local signing is supported on macOS only." >&2
  exit 1
fi

signing_dir="$HOME/Library/Application Support/Parler Build Signing"
certificate="$signing_dir/certificate.pem"
keychain="$HOME/Library/Keychains/login.keychain-db"
identity_name="Parler Local Development"
mkdir -p "$signing_dir"
chmod 700 "$signing_dir"

# Recover the public certificate if only the local metadata file was removed.
if [[ ! -f "$certificate" ]]; then
  existing_certificate=$(security find-certificate -c "$identity_name" -p "$keychain" 2>/dev/null || true)
  if [[ -n "$existing_certificate" ]]; then
    printf '%s\n' "$existing_certificate" > "$certificate"
    chmod 600 "$certificate"
  fi
fi

if [[ -f "$certificate" ]]; then
  fingerprint=$(openssl x509 -in "$certificate" -noout -fingerprint -sha1 | cut -d= -f2 | tr -d ':')
  identities=$(security find-identity -v -p codesigning "$keychain")
  if [[ "$identities" == *"$fingerprint"* ]]; then
    echo "$fingerprint"
    exit 0
  fi
  echo "The existing Parler signing certificate has no valid keychain identity." >&2
  echo "Restore its original keychain identity; do not regenerate it or permissions may be lost." >&2
  exit 1
fi

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/parler-signing.XXXXXX")
chmod 700 "$work_dir"
trap 'rm -rf "$work_dir"' EXIT
cat > "$work_dir/openssl.cnf" <<'EOF'
[req]
distinguished_name = subject
x509_extensions = signing
prompt = no
[subject]
CN = Parler Local Development
[signing]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid:always
EOF

openssl req -new -x509 -newkey rsa:2048 -nodes -days 3650 \
  -config "$work_dir/openssl.cnf" \
  -keyout "$work_dir/private.pem" -out "$work_dir/certificate.pem" 2>/dev/null
openssl rand -hex 32 > "$work_dir/password"
openssl pkcs12 -export -legacy -name "$identity_name" \
  -inkey "$work_dir/private.pem" -in "$work_dir/certificate.pem" \
  -out "$work_dir/identity.p12" -passout "file:$work_dir/password"
# Restrict private-key access to codesign; never allow all applications (-A).
security import "$work_dir/identity.p12" -k "$keychain" -x \
  -P "$(cat "$work_dir/password")" -T /usr/bin/codesign >&2
# Trust this identity for code signing only, not TLS or other certificate uses.
security add-trusted-cert -r trustRoot -p codeSign -k "$keychain" "$work_dir/certificate.pem"
cp "$work_dir/certificate.pem" "$certificate"
chmod 600 "$certificate"
fingerprint=$(openssl x509 -in "$certificate" -noout -fingerprint -sha1 | cut -d= -f2 | tr -d ':')
identities=$(security find-identity -v -p codesigning "$keychain")
if [[ "$identities" != *"$fingerprint"* ]]; then
  echo "The new identity is not yet trusted for code signing. Check Keychain Access." >&2
  exit 1
fi
echo "$fingerprint"
