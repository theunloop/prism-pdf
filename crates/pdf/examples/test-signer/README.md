# Throwaway test signer

`cert.der` (X.509) and `key.der` (PKCS#8, RSA-2048) are a **single-purpose, self-signed test
keypair** used only by the `gen_pdfa` example to produce the signed PDF/A sample
(`corpus/prismpdf-pdfa/prismpdf-signed-*-pass.pdf`).

**Do not trust this key for anything.** The private key is committed in the clear on purpose — it
signs nothing of value and exists so the signed corpus file is reproducible byte-for-byte (RSA
PKCS#1 v1.5 is deterministic; the example also pins the signing time). It is generated once and
embedded via `include_bytes!`.

Regenerated (if ever needed) with:

```sh
openssl req -x509 -newkey rsa:2048 -keyout key.pem -out cert.pem -days 36500 -nodes \
  -subj "/CN=Prism PDF Test Signer (throwaway, do not trust)" -sha256
openssl pkcs8 -topk8 -nocrypt -in key.pem -outform DER -out key.der
openssl x509 -in cert.pem -outform DER -out cert.der
rm key.pem cert.pem
```

Regenerating changes the signed sample's bytes; re-run `gen_pdfa` and re-validate.
