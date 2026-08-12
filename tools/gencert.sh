#!/bin/sh
# Self-signed cert for the local backend. The AC EVO client accepts it.
# MSYS_NO_PATHCONV=1 is required in Git Bash, or -subj is mangled into a
# Windows path and openssl rejects it.
cd "$(dirname "$0")"
MSYS_NO_PATHCONV=1 openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout key.pem -out cert.pem -days 3650 -subj "/CN=localhost"
echo "wrote cert.pem + key.pem"
