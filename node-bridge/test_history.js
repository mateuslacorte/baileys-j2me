const { default: makeWASocket, useMultiFileAuthState } = require('@whiskeysockets/baileys');
const pino = require('pino');
const path = require('path');

const sessionDir = path.join(__dirname, '..', 'sessions', 'bb_bold_9900');

async function test() {
    const { state } = await useMultiFileAuthState(sessionDir);
    const sock = makeWASocket({
        auth: state,
        logger: pino({ level: 'silent' }),
        syncFullHistory: false
    });

    sock.ev.on('connection.update', (u) => {
        console.log('connection:', u.connection);
    });

    sock.ev.on('messaging-history.set', ({ chats, contacts, messages }) => {
        console.log('messaging-history.set -> chats:', chats?.length, 'contacts:', contacts?.length, 'messages:', messages?.length);
        if (chats && chats.length > 0) {
            console.log('Sample chats:', chats.slice(0, 5).map(c => ({ id: c.id, name: c.name })));
        }
    });

    sock.ev.on('chats.upsert', (c) => {
        console.log('chats.upsert:', c?.length);
    });

    sock.ev.on('messages.upsert', (m) => {
        console.log('messages.upsert:', m?.messages?.length);
    });
}

test();
