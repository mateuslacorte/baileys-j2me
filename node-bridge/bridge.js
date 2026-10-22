const { default: makeWASocket, useMultiFileAuthState } = require('@whiskeysockets/baileys');
const pino = require('pino');
const fs = require('fs');
const path = require('path');
const FileChatStore = require('./store');
const { safeSend } = require('./utils');
const { registerEvents } = require('./events');
const { registerCommands } = require('./commands');

process.stdout.on('error', (err) => {
    if (err.code === 'EPIPE') process.exit(0);
});

const deviceId = process.argv[2] || 'default_device';
const sessionDir = path.join(__dirname, '..', 'sessions', deviceId);

if (!fs.existsSync(sessionDir)) {
    fs.mkdirSync(sessionDir, { recursive: true });
}

const store = new FileChatStore(sessionDir);

async function startBridge() {
    const { state, saveCreds } = await useMultiFileAuthState(sessionDir);

    const sock = makeWASocket({
        auth: state,
        printQRInTerminal: false,
        logger: pino({ level: 'silent' }),
        syncFullHistory: false
    });

    registerEvents(sock, {
        deviceId,
        sessionDir,
        store,
        saveCreds,
        onReconnect: startBridge
    });

    registerCommands(sock, {
        deviceId,
        store
    });
}

startBridge().catch(err => {
    safeSend({ type: 'error', error: err.message });
    process.exit(1);
});
