# Baileys-J2ME Multi-Device Server

A complete bridging architecture connecting vintage J2ME (running MIDP 2.0 / CLDC 1.1) and BlackBerry OS devices to modern WhatsApp on Android and iOS using Baileys.

---

## 🌟 Key Features

1. **Native WhatsApp Web Pairing via QR Code**:
   - First-time app startup renders a live QR Code fetched directly from the Baileys bridge.
   - Simply scan from WhatsApp on Android or iPhone (**Linked Devices** / **Aparelhos Conectados**).

2. **Multi-Device Session Isolation**:
   - Each J2ME device has a unique `device_id` (e.g., `wpp_random_id`).
   - The Rust server manages independent persistent sessions in `sessions/<device_id>/` so each device only configures once.

3. **High-Efficiency Retro Protocol**:
   - Long-polling sync endpoint designed for low memory and intermittent mobile networks (GPRS/EDGE/3G).
   - Dynamic server-side image resizing and ASCII-escaped JSON responses for vintage Unicode parsers.

---

## 📋 Prerequisites

Before compiling and running the server, make sure you have the following installed:

1. **Rust & Cargo** (v1.70+ or stable):
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```
2. **Node.js & npm** (v18+ or v20+ LTS):
   - The server uses Node.js to run `@whiskeysockets/baileys` companion instances.
   ```bash
   # Debian / Ubuntu:
   sudo apt update && sudo apt install -y nodejs npm
   ```
3. **OpenSSL** (for SSL certificate generation):
   ```bash
   sudo apt install -y openssl
   ```
4. **Nginx** (recommended reverse proxy for handling TLS termination and legacy device quirks).

---

## 🚀 How to Compile and Run the Server

### 1. Clone the Repository
```bash
git clone https://github.com/mateuslacorte/baileys-j2me
cd baileys-j2me
```

### 2. Install Node.js Bridge Dependencies
The Rust server spawns a Node.js companion process located in `node-bridge/` to interact with the WhatsApp protocol via Baileys. You must install its dependencies prior to starting:
```bash
cd node-bridge
npm install
cd ..
```

### 3. Run in Development Mode
```bash
cargo run
```
The server will start listening on `0.0.0.0:8085` by default.

### 4. Compile for Production (Release Mode)
To compile a release binary:
```bash
cargo build --release
```
The compiled binary will be placed at `./target/release/baileys-j2me`. You can execute it directly:
```bash
./target/release/baileys-j2me
```

### Environment Variables
You can configure the server using environment variables:
| Variable | Default | Description |
|---|---|---|
| `PORT` | `8085` | Port the Rust HTTP server binds to. |
| `NODE_PATH` | `node` | Path to the `node` executable (e.g. `/usr/bin/node` or nvm path). |

### 5. Production Service (systemd)
On Linux servers (e.g., Debian/Ubuntu), you can set up a persistent systemd unit:

Create `/etc/systemd/system/baileys-j2me.service`:
```ini
[Unit]
Description=Baileys J2ME Bridge
After=network.target

[Service]
Type=simple
User=youruser
WorkingDirectory=/path/to/baileys-j2me
ExecStart=/path/to/baileys-j2me/target/release/baileys-j2me
Restart=always
RestartSec=5
Environment=PORT=8085
Environment=PATH=/usr/bin:/bin:/usr/local/bin
Environment=NODE_PATH=/usr/bin/node

[Install]
WantedBy=multi-user.target
```

Enable and start the service:
```bash
sudo systemctl daemon-reload
sudo systemctl enable --now baileys-j2me
sudo systemctl status baileys-j2me
```

---

## 🔒 Certificate & TLS Quirks for Vintage Handsets

Vintage mobile operating systems (such as **BlackBerry OS 5/6/7**, **Symbian S60**, and generic **J2ME MIDP 2.0 / CLDC 1.1** devices) were built around cryptographic standards from the 2000s and early 2010s. Modern web servers, default OpenSSL configs, and CDN proxies (like Cloudflare) will completely fail TLS handshakes with these handsets unless configured specifically.

### 1. Binary DER (`.cer` / `.der`) vs Base64 PEM (`.crt` / `.pem`)
- **Quirk**: Most J2ME and BlackBerry certificate stores cannot parse ASCII-armored PEM files (`-----BEGIN CERTIFICATE-----`). If you serve a `.pem` or `.crt`, the phone may show *"Unsupported certificate format"* or fail to import the root authority.
- **Solution**: Convert your certificate to binary **DER** format (`.cer` or `.der`).
- **MIME Type Requirement**: Nginx must serve `.cer` and `.der` files with the exact MIME type `application/x-x509-ca-cert` to trigger the device's native certificate installation wizard.

### 2. Root Authority Requirement (`basicConstraints = critical, CA:TRUE`)
- **Quirk**: When generating a self-signed or custom certificate for vintage handsets, the certificate **must** explicitly contain the X.509 v3 extension `CA:TRUE`. If missing, the handset will reject the certificate or warn on every single HTTPS connection request.
- **Solution**: Include `basicConstraints = critical, CA:TRUE` and `keyCertSign` in the key usage.

### 3. Legacy TLS Protocols (TLS 1.0 & TLS 1.1)
- **Quirk**: Vintage phones do not support TLS 1.3, and many only support TLS 1.0 or TLS 1.1 (BlackBerry OS 7 supports TLS 1.2, but without modern elliptic curves or AEAD ciphers).
- **Solution**: The reverse proxy must explicitly enable legacy TLS versions:
  ```nginx
  ssl_protocols TLSv1 TLSv1.1 TLSv1.2 TLSv1.3;
  ```

### 4. OpenSSL 3.x Security Level Override (`@SECLEVEL=0`)
- **Quirk**: Modern Linux distributions (Debian 12+, Ubuntu 22.04+) enforce OpenSSL Security Level 2 by default. Under Level 2 (and Level 1), OpenSSL **prohibits** TLS 1.0, TLS 1.1, and legacy ciphers (such as SHA-1, 1024-bit RSA, and CBC ciphers), throwing `SSL_ERROR_UNSUPPORTED_PROTOCOL` or handshake failure.
- **Solution**: Force Nginx to allow legacy ciphers by adding `@SECLEVEL=0`:
  ```nginx
  ssl_prefer_server_ciphers on;
  ssl_ciphers "ALL:@SECLEVEL=0";
  ```

### 5. Direct IP / DNS-Only (Bypass Cloudflare Proxy)
- **Quirk**: Modern CDN / Edge proxies (e.g. Cloudflare) terminate TLS at their edge using modern TLS 1.2/1.3 standards and drop legacy handshakes.
- **Solution**: The subdomain or IP used by retro devices (e.g. `wpp.j2me.lacorte.dev`) must be **DNS-only** (Grey Cloud / no proxy), pointing directly to your origin server.

---

## 🛠️ Step-by-Step Certificate Generation & Nginx Configuration

### 1. Generating a Compatible Self-Signed Root Certificate
Create a configuration file `cert.cnf` with Subject Alternative Names (SAN) and the `CA:TRUE` constraint:

```ini
[req]
distinguished_name = req_distinguished_name
x509_extensions = v3_ca
prompt = no

[req_distinguished_name]
C = BR
ST = SP
L = Sao Paulo
O = RetroTech
OU = WhatsApp J2ME
CN = wpp.yourdomain.com

[v3_ca]
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid:always,issuer
basicConstraints = critical, CA:TRUE, pathlen:0
keyUsage = critical, digitalSignature, keyEncipherment, keyCertSign, cRLSign
extendedKeyUsage = serverAuth, clientAuth
subjectAltName = @alt_names

[alt_names]
DNS.1 = wpp.yourdomain.com
DNS.2 = yourdomain.com
IP.1 = 192.168.1.100 # Your server IP
```

Generate the private key and certificate:
```bash
# 1. Generate RSA 2048 private key and PEM certificate
openssl req -x509 -new -nodes -keyout wpp-self-key -out wpp-self-cert.crt -days 3650 -config cert.cnf

# 2. Convert to binary DER format for BlackBerry and J2ME OTA installation
openssl x509 -in wpp-self-cert.crt -outform DER -out cert.cer
cp cert.cer cert.der
```

### 2. Create the API username and password

Nginx enforces HTTP Basic Auth in front of the bridge; no Rust or Node changes
are required. On Debian/Ubuntu, create a password file outside the web root:

```bash
sudo apt install -y apache2-utils
sudo htpasswd -c /etc/nginx/baileys.htpasswd bridge-user
sudo chown root:www-data /etc/nginx/baileys.htpasswd
sudo chmod 640 /etc/nginx/baileys.htpasswd
```

`htpasswd` prompts for the password without putting it in shell history. Replace
`bridge-user` with your username. Use printable ASCII credentials, with no colon
in the username, to match the J2ME compiler. To change the password or add another
user, run `sudo htpasswd /etc/nginx/baileys.htpasswd bridge-user` without `-c`
(which would replace the file). Adjust `www-data` for your Nginx worker group.

Keep port 8085 blocked from external access so clients cannot bypass Nginx. Use
HTTPS in the phone build: Basic Auth encodes credentials and depends on TLS for
transport protection. The HTTP virtual host below serves certificates and app
files but redirects API requests to HTTPS.

### 3. Sample Nginx Reverse Proxy Configuration
Below is a battle-tested Nginx virtual host configuration tailored for vintage handsets:

```nginx
# Retro Mirror & API Server for Vintage Devices
server {
    listen 80;
    listen [::]:80;
    server_name wpp.yourdomain.com;

    root /var/www/j2me;
    client_max_body_size 50M;

    # Serve certificates with the required MIME type for native installation
    location ~ ^/cert\.(crt|cer|der|pem)$ {
        default_type application/x-x509-ca-cert;
        types {
            application/x-x509-ca-cert crt cer der pem;
        }
        add_header Cache-Control "no-cache, no-store, must-revalidate";
    }

    # Serve application binaries
    location ~ ^/WhatsApp\.(jar|jad)$ {
        types {
            text/vnd.sun.j2me.app-descriptor jad;
            application/java-archive jar;
        }
        add_header Cache-Control "no-cache, no-store, must-revalidate";
    }

    # API clients must use HTTPS, including for their first request.
    location / {
        return 301 https://$host$request_uri;
    }
}

server {
    listen 443 ssl default_server;
    listen [::]:443 ssl default_server;
    server_name wpp.yourdomain.com;

    ssl_certificate /etc/nginx/ssl/wpp-self-cert.crt;
    ssl_certificate_key /etc/nginx/ssl/wpp-self-key;

    # Vintage TLS protocols and ciphers
    ssl_protocols TLSv1 TLSv1.1 TLSv1.2 TLSv1.3;
    ssl_prefer_server_ciphers on;
    ssl_ciphers "ALL:@SECLEVEL=0";

    root /var/www/j2me;
    client_max_body_size 50M;

    location ~ ^/cert\.(crt|cer|der|pem)$ {
        default_type application/x-x509-ca-cert;
        types {
            application/x-x509-ca-cert crt cer der pem;
        }
        add_header Cache-Control "no-cache, no-store, must-revalidate";
    }

    location ~ ^/WhatsApp\.(jar|jad)$ {
        types {
            text/vnd.sun.j2me.app-descriptor jad;
            application/java-archive jar;
        }
        add_header Cache-Control "no-cache, no-store, must-revalidate";
    }

    location / {
        auth_basic "WhatsApp J2ME bridge";
        auth_basic_user_file /etc/nginx/baileys.htpasswd;
        proxy_set_header Authorization "";
        proxy_pass http://127.0.0.1:8085/;
        proxy_http_version 1.1;
        proxy_buffering off;
        proxy_read_timeout 86400s;
        proxy_send_timeout 86400s;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $http_connection;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

Validate and reload Nginx, then check both cases:

```bash
sudo nginx -t && sudo systemctl reload nginx
# Expect 401 without credentials:
curl --cacert /etc/nginx/ssl/wpp-self-cert.crt -o /dev/null -w '%{http_code}\n' https://wpp.yourdomain.com/api/device/setup-check/status
# Prompts for the password; expect 200 and JSON:
curl --cacert /etc/nginx/ssl/wpp-self-cert.crt --user bridge-user https://wpp.yourdomain.com/api/device/setup-check/status
```

The authenticated check creates a test device session; do not pair it. If using
an API subpath such as `/server/whatsapp/`, put the same `auth_basic` directives
in that proxy location and configure the matching path in the client.

---

## 📡 REST & Long-Polling Endpoints

The Rust server provides the following endpoints for J2ME client interaction:

| Method | Endpoint | Description |
|---|---|---|
| `GET` | `/api/device/:device_id/status` | Current connection status (`initializing`, `qr_ready`, `connected`). |
| `GET` | `/api/device/:device_id/qr.png` | Serves the dynamic QR Code image rendered directly as a 200x200 PNG for J2ME screens. |
| `GET` | `/api/device/:device_id/chats` | Lists synced conversation threads and chat previews. |
| `GET` | `/api/device/:device_id/messages` | Historical messages for a specific chat (`?jid=...&limit=...`). |
| `POST` | `/api/device/:device_id/send` | Send a text message (`{"to": "...@s.whatsapp.net", "text": "..."}`). |
| `POST` | `/api/device/:device_id/send_audio` | Send a voice note encoded in AMR or audio format. |
| `POST` | `/api/device/:device_id/mark_read` | Mark messages from a contact/group as read. |
| `GET` | `/api/device/:device_id/sync` | High-efficiency long-polling sync route for real-time message updates and heartbeats. |
