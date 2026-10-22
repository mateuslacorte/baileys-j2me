const { default: makeWASocket, useMultiFileAuthState, DisconnectReason, downloadMediaMessage } = require('@whiskeysockets/baileys');
const pino = require('pino');
const readline = require('readline');
const fs = require('fs');
const path = require('path');
const FileChatStore = require('./store');

process.stdout.on('error', (err) => {
    if (err.code === 'EPIPE') process.exit(0);
});

const deviceId = process.argv[2] || 'default_device';
const sessionDir = path.join(__dirname, '..', 'sessions', deviceId);

if (!fs.existsSync(sessionDir)) {
    fs.mkdirSync(sessionDir, { recursive: true });
}

const store = new FileChatStore(sessionDir);

function safeSend(obj) {
    try {
        process.stdout.write(JSON.stringify(obj) + '\n');
    } catch (e) {}
}

function cleanJid(jid) {
    if (!jid) return '';
    const atIdx = jid.indexOf('@');
    if (atIdx === -1) return jid;
    let user = jid.substring(0, atIdx);
    const domain = jid.substring(atIdx);
    const colonIdx = user.indexOf(':');
    if (colonIdx !== -1) {
        user = user.substring(0, colonIdx);
    }
    return user + domain;
}

function normalizeJid(jid) {
    return store.canonicalJid(cleanJid(jid));
}

function formatPhoneNumber(raw) {
    if (!raw) return '';
    const s = String(raw).trim();
    if (s.includes('@g.us') || (s.includes('-') && s.includes('@'))) return s;
    if (/[a-zA-Z]/.test(s)) return s;
    if (s.startsWith('+') && s.includes(' ')) return s;
    const digits = s.replace(/\D/g, '');
    const len = digits.length;
    if (digits.startsWith('55')) {
        if (len === 13) return `+55 ${digits.substring(2, 4)} ${digits.substring(4, 9)}-${digits.substring(9, 13)}`;
        if (len === 12) return `+55 ${digits.substring(2, 4)} ${digits.substring(4, 8)}-${digits.substring(8, 12)}`;
    } else if (len === 11 && !digits.startsWith('1')) {
        return `+55 ${digits.substring(0, 2)} ${digits.substring(2, 7)}-${digits.substring(7, 11)}`;
    } else if (len === 10 && !digits.startsWith('1')) {
        return `+55 ${digits.substring(0, 2)} ${digits.substring(2, 6)}-${digits.substring(6, 10)}`;
    }
    if (digits.startsWith('1') && len === 11) {
        return `+1 ${digits.substring(1, 4)} ${digits.substring(4, 7)}-${digits.substring(7, 11)}`;
    }
    if (len >= 7 && len <= 13) {
        return `+${digits}`;
    }
    return s;
}

function resolveMentions(text) {
    if (!text || typeof text !== 'string') return text || '';
    if (!text.includes('@')) return text;
    return text.replace(/@(\d{7,25})/g, (match, digits) => {
        const lidJid = digits + '@lid';
        let phoneJid = store.lidMap ? (store.lidMap.get ? store.lidMap.get(lidJid) : store.lidMap[lidJid]) : null;
        let phone = phoneJid ? phoneJid.split('@')[0] : (digits.length <= 13 && !digits.startsWith('2') ? digits : null);
        let contact = null;
        if (phoneJid && store.contacts.has(phoneJid)) contact = store.contacts.get(phoneJid);
        if (!contact && phone && store.contacts.has(phone)) contact = store.contacts.get(phone);
        if (!contact && store.contacts.has(digits + '@s.whatsapp.net')) contact = store.contacts.get(digits + '@s.whatsapp.net');
        if (!contact && store.contacts.has(digits)) contact = store.contacts.get(digits);

        if (contact && /[a-zA-Z]/.test(contact) && contact !== '~') {
            return '@' + contact.trim();
        }
        if (phone) {
            return '@' + formatPhoneNumber(phone);
        }
        return '@Participante';
    });
}

function getBestName(rawJid, fallback) {
    if (!rawJid) return fallback || '';
    const nJid = normalizeJid(rawJid);
    if (store.contacts.has(nJid)) {
        const c = store.contacts.get(nJid);
        if (c && /[a-zA-Z]/.test(c) && c !== '~') return c.trim();
    }
    if (store.contacts.has(rawJid)) {
        const c = store.contacts.get(rawJid);
        if (c && /[a-zA-Z]/.test(c) && c !== '~') return c.trim();
    }
    const user = nJid.split('@')[0];
    if (store.contacts.has(user)) {
        const c = store.contacts.get(user);
        if (c && /[a-zA-Z]/.test(c) && c !== '~') return c.trim();
    }
    if (fallback && fallback.trim().length > 0 && !fallback.trim().endsWith('@lid') && !/^\d{14,}$/.test(fallback.trim())) {
        return fallback.trim();
    }
    if (nJid.endsWith('@lid') || (user.length >= 14 && user.startsWith('2'))) {
        return 'Participante';
    }
    if (/^\d{7,13}$/.test(user)) {
        return formatPhoneNumber(user);
    }
    return user;
}

function extractText(message) {
    if (!message) return '';
    if (message.ephemeralMessage?.message) message = message.ephemeralMessage.message;
    if (message.viewOnceMessage?.message) message = message.viewOnceMessage.message;
    if (message.viewOnceMessageV2?.message) message = message.viewOnceMessageV2.message;
    if (message.documentWithCaptionMessage?.message) message = message.documentWithCaptionMessage.message;
    return message.conversation ||
        message.extendedTextMessage?.text ||
        message.imageMessage?.caption ||
        (message.imageMessage ? '[Imagem]' : '') ||
        (message.audioMessage ? '[Audio]' : '') ||
        message.videoMessage?.caption ||
        (message.videoMessage ? '[Vídeo]' : '') ||
        message.documentMessage?.title ||
        (message.documentMessage ? '[Documento]' : '') ||
        (message.stickerMessage ? '[Figurinha]' : '') ||
        '';
}

function extractQuoted(message, remoteJid) {
    if (!message) return null;
    if (message.ephemeralMessage?.message) message = message.ephemeralMessage.message;
    if (message.viewOnceMessage?.message) message = message.viewOnceMessage.message;
    if (message.viewOnceMessageV2?.message) message = message.viewOnceMessageV2.message;
    if (message.documentWithCaptionMessage?.message) message = message.documentWithCaptionMessage.message;

    const ctx = message.extendedTextMessage?.contextInfo ||
                message.imageMessage?.contextInfo ||
                message.videoMessage?.contextInfo ||
                message.audioMessage?.contextInfo ||
                message.documentMessage?.contextInfo ||
                message.stickerMessage?.contextInfo;
    if (!ctx || !ctx.stanzaId) return null;
    const qMsg = ctx.quotedMessage;
    let quotedText = resolveMentions(extractText(qMsg)) || '';
    let sender = ctx.participant ? normalizeJid(ctx.participant) : '';
    let senderName = sender ? getBestName(sender, '') : '';

    if (!quotedText && ctx.stanzaId) {
        const chatMsgs = store.messages.get(normalizeJid(remoteJid || ''));
        if (chatMsgs) {
            const found = chatMsgs.find(m => m.id === ctx.stanzaId);
            if (found) {
                quotedText = resolveMentions(found.text);
                if (!senderName) {
                    senderName = found.fromMe ? 'Você' : (found.senderName || getBestName(found.participant || '', ''));
                }
            }
        }
    }

    if (!senderName && remoteJid) {
        senderName = getBestName(normalizeJid(remoteJid), '');
    }

    return {
        id: ctx.stanzaId,
        sender: senderName || 'Mensagem',
        text: quotedText
    };
}

function extractImageMessage(message) {
    if (!message) return null;
    if (message.ephemeralMessage?.message) return extractImageMessage(message.ephemeralMessage.message);
    if (message.viewOnceMessage?.message) return extractImageMessage(message.viewOnceMessage.message);
    if (message.viewOnceMessageV2?.message) return extractImageMessage(message.viewOnceMessageV2.message);
    if (message.documentWithCaptionMessage?.message) return extractImageMessage(message.documentWithCaptionMessage.message);
    return message.imageMessage || null;
}

function saveMediaIfImage(msg, sessionDir) {
    try {
        const imgMsg = extractImageMessage(msg.message);
        if (!imgMsg) return false;
        const id = msg.key?.id;
        if (!id) return false;

        const mediaDir = path.join(sessionDir, 'media');
        if (!fs.existsSync(mediaDir)) {
            fs.mkdirSync(mediaDir, { recursive: true });
        }
        const filePath = path.join(mediaDir, `${id}.jpg`);

        if (!fs.existsSync(filePath) && imgMsg.jpegThumbnail) {
            try {
                fs.writeFileSync(filePath, Buffer.from(imgMsg.jpegThumbnail));
            } catch (e) {}
        }

        downloadMediaMessage(msg, 'buffer', {}, { logger: pino({ level: 'silent' }) })
            .then(buf => {
                if (buf && buf.length > 0) {
                    fs.writeFileSync(filePath, buf);
                }
            })
            .catch(() => {});
        return true;
    } catch (e) {
        return false;
    }
}

async function startBridge() {
    const { state, saveCreds } = await useMultiFileAuthState(sessionDir);

    const sock = makeWASocket({
        auth: state,
        printQRInTerminal: false,
        logger: pino({ level: 'silent' }),
        syncFullHistory: false
    });

    sock.ev.on('creds.update', saveCreds);

    sock.ev.on('connection.update', async (update) => {
        const { connection, lastDisconnect, qr } = update;
        if (qr) {
            safeSend({ type: 'qr', device_id: deviceId, qr: qr });
        }
        if (connection === 'close') {
            const shouldReconnect = (lastDisconnect?.error)?.output?.statusCode !== DisconnectReason.loggedOut;
            safeSend({ type: 'connection', device_id: deviceId, status: 'close', reconnect: shouldReconnect });
            if (shouldReconnect) {
                setTimeout(startBridge, 3000);
            }
        } else if (connection === 'open') {
            safeSend({ type: 'connection', device_id: deviceId, status: 'open', user: sock.user });

            // Refresh LID mappings from session files
            store.loadLidMappings();

            const snap = store.getSnapshot(40);
            if (snap.chats.length > 0) {
                safeSend({
                    type: 'snapshot',
                    device_id: deviceId,
                    chats: snap.chats,
                    messages: []
                });
            }

            // Automatically request history and placeholder resend for groups that have messages with missing participants
            setTimeout(async () => {
                console.error('[bridge] Starting syncGroupsMissingParticipants...');
                for (const [jid, msgs] of store.messages) {
                    if (jid.includes('@g.us') && Array.isArray(msgs) && msgs.length > 0) {
                        const missing = msgs.filter(m => !m.fromMe && !m.participant && m.id);
                        if (missing.length > 0) {
                            if (sock.requestPlaceholderResend) {
                                for (let i = 0; i < missing.length; i++) {
                                    try {
                                        sock.requestPlaceholderResend({
                                            remoteJid: jid,
                                            id: missing[i].id,
                                            fromMe: false
                                        });
                                    } catch (e) {}
                                    if (i % 15 === 0) {
                                        await new Promise(r => setTimeout(r, 500));
                                    }
                                }
                            }
                            if (sock.fetchMessageHistory) {
                                try {
                                    const cursor = msgs[msgs.length - 1];
                                    await sock.fetchMessageHistory(50, {
                                        remoteJid: jid,
                                        id: cursor.id,
                                        fromMe: Boolean(cursor.fromMe)
                                    }, cursor.timestamp || Date.now());
                                    await new Promise(r => setTimeout(r, 1000));
                                } catch (e) {}
                            }
                        }
                    }
                }
            }, 3000);
        }
    });

    // Populate contacts
    sock.ev.on('contacts.upsert', (contacts) => {
        if (!contacts || !Array.isArray(contacts)) return;
        const list = [];
        for (const c of contacts) {
            if (!c || !c.id || c.id === 'status@broadcast') continue;
            const cId = cleanJid(c.id);
            const name = c.name || c.notify || c.verifiedName || '';
            if (c.lid) {
                const cLid = cleanJid(c.lid);
                store.lidMap.set(cLid, cId);
                store.phoneToLid.set(cId, cLid);
                if (name) store.contacts.set(cLid, name);
            }
            if (name) {
                store.contacts.set(cId, name);
                store.contacts.set(c.id, name);
            }
            const targetJid = normalizeJid(cId);
            store.upsertChat(targetJid, name || targetJid.split('@')[0]);
            list.push({ jid: targetJid, name: name || targetJid.split('@')[0] });
        }
        store.save();
        if (list.length > 0) {
            safeSend({
                type: 'chats_update',
                device_id: deviceId,
                chats: list
            });
        }
    });

    sock.ev.on('chats.upsert', (chats) => {
        if (!chats || !Array.isArray(chats)) return;
        const list = [];
        for (const ch of chats) {
            if (!ch || !ch.id || ch.id === 'status@broadcast') continue;
            const targetJid = normalizeJid(ch.id);
            const name = ch.name || getBestName(ch.id, '');
            if (ch.name) store.contacts.set(targetJid, ch.name);
            store.upsertChat(targetJid, name, ch.unreadCount || 0);
            list.push({
                jid: targetJid,
                name: name,
                unread: ch.unreadCount || 0
            });
        }
        store.save();
        if (list.length > 0) {
            safeSend({
                type: 'chats_update',
                device_id: deviceId,
                chats: list
            });
        }
    });

    sock.ev.on('groups.update', (updates) => {
        if (!updates || !Array.isArray(updates)) return;
        const list = [];
        for (const update of updates) {
            if (update && update.id && update.subject) {
                const targetJid = normalizeJid(update.id);
                store.contacts.set(targetJid, update.subject);
                store.upsertChat(targetJid, update.subject);
                list.push({ jid: targetJid, name: update.subject });
            }
        }
        store.save();
        if (list.length > 0) {
            safeSend({
                type: 'chats_update',
                device_id: deviceId,
                chats: list
            });
        }
    });

    sock.ev.on('messaging-history.set', ({ chats, contacts, messages }) => {
        console.error('[bridge] messaging-history.set event! chats=' + (chats?.length || 0) + ' msgs=' + (messages?.length || 0));
        if (messages && messages.length > 0) {
            const first = messages[0];
            console.error('[bridge] Sample msg from history.set: ' + JSON.stringify({
                key: first.key,
                participant: first.participant,
                pushName: first.pushName
            }));
        }
        store.loadLidMappings();

        if (contacts && Array.isArray(contacts)) {
            for (const c of contacts) {
                if (!c || !c.id) continue;
                const cId = cleanJid(c.id);
                const name = c.name || c.notify || c.verifiedName || '';
                if (c.lid) {
                    const cLid = cleanJid(c.lid);
                    store.lidMap.set(cLid, cId);
                    store.phoneToLid.set(cId, cLid);
                    if (name) store.contacts.set(cLid, name);
                }
                if (name) {
                    store.contacts.set(cId, name);
                    store.contacts.set(c.id, name);
                }
            }
        }

        if (chats && Array.isArray(chats)) {
            for (const ch of chats) {
                if (!ch || !ch.id || ch.id === 'status@broadcast') continue;
                const targetJid = normalizeJid(ch.id);
                const name = ch.name || getBestName(ch.id, '');
                if (ch.name) store.contacts.set(targetJid, ch.name);
                store.upsertChat(targetJid, name, ch.unreadCount || 0);
            }
        }

        if (messages && Array.isArray(messages)) {
            for (const msg of messages) {
                if (!msg || !msg.message || !msg.key) continue;
                const rawRemote = msg.key.remoteJid;
                if (!rawRemote || rawRemote === 'status@broadcast') continue;
                const rawText = extractText(msg.message);
                if (!rawText) continue;
                const text = resolveMentions(rawText);

                const remoteJid = normalizeJid(rawRemote);
                const fromMe = Boolean(msg.key.fromMe);
                const id = msg.key.id || ('msg_' + Date.now());
                const ts = (msg.messageTimestamp ? Number(msg.messageTimestamp) * 1000 : Date.now());

                const isGroup = rawRemote.includes('@g.us');
                let rawPart = msg.key?.participant || msg.participant || msg.key?.participantAlt || '';
                let participant = rawPart ? cleanJid(rawPart) : '';
                if (!participant && !isGroup && !fromMe) {
                    participant = cleanJid(rawRemote);
                }
                if (participant) {
                    participant = store.canonicalJid(participant);
                }
                const senderName = msg.pushName ? msg.pushName.trim() : '';

                if (!fromMe && senderName) {
                    if (isGroup && participant) {
                        store.contacts.set(participant, senderName);
                        if (rawPart) store.contacts.set(cleanJid(rawPart), senderName);
                    } else if (!isGroup) {
                        store.contacts.set(remoteJid, senderName);
                        store.contacts.set(cleanJid(rawRemote), senderName);
                        store.upsertChat(remoteJid, senderName);
                    }
                }

                const quoted = extractQuoted(msg.message, rawRemote);
                const imgMsg = extractImageMessage(msg.message);
                const msgData = {
                    id: id,
                    fromMe: fromMe,
                    text: text,
                    timestamp: ts
                };
                if (imgMsg) {
                    msgData.hasImage = true;
                    saveMediaIfImage(msg, sessionDir);
                }
                if (isGroup && !fromMe) {
                    if (participant) msgData.participant = participant;
                    msgData.senderName = senderName || (participant ? getBestName(participant, '') : '');
                }
                if (quoted) {
                    msgData.quotedId = quoted.id;
                    msgData.quotedText = quoted.text;
                    msgData.quotedSender = quoted.sender;
                }

                store.addMessage(remoteJid, msgData);
            }
        }

        store.save();
        const snap = store.getSnapshot(40);
        safeSend({
            type: 'snapshot',
            device_id: deviceId,
            chats: snap.chats,
            messages: []
        });
    });

    sock.ev.on('messages.upsert', async (m) => {
        if (!m || !m.messages || !Array.isArray(m.messages)) return;
        for (const msg of m.messages) {
            if (!msg || !msg.message || !msg.key) continue;
            const rawRemote = msg.key.remoteJid;
            if (!rawRemote || rawRemote === 'status@broadcast') continue;

            const remoteJid = normalizeJid(rawRemote);
            const fromMe = Boolean(msg.key.fromMe);
            const id = msg.key.id || ('msg_' + Date.now());
            const timestamp = (msg.messageTimestamp ? Number(msg.messageTimestamp) * 1000 : Date.now());

            const isGroup = rawRemote.includes('@g.us');
            let rawPart = msg.key?.participant || msg.participant || msg.key?.participantAlt || '';
            let participant = rawPart ? cleanJid(rawPart) : '';
            if (!participant && !isGroup && !fromMe) {
                participant = cleanJid(rawRemote);
            }
            if (participant) {
                participant = store.canonicalJid(participant);
            }
            const senderName = msg.pushName ? msg.pushName.trim() : '';

            // Update existing message in store even if text is absent (e.g. placeholder resend)
            const existingList = store.messages.get(remoteJid);
            if (existingList) {
                const ex = existingList.find(m => m.id === id);
                if (ex) {
                    if (participant && !ex.participant) ex.participant = participant;
                    if (senderName && !ex.senderName) ex.senderName = senderName;
                    store.save();
                }
            }

            const rawText = extractText(msg.message);
            if (!rawText) continue;
            const text = resolveMentions(rawText);

            if (!fromMe && senderName) {
                if (isGroup && participant) {
                    store.contacts.set(participant, senderName);
                    if (rawPart) store.contacts.set(cleanJid(rawPart), senderName);
                } else if (!isGroup) {
                    store.contacts.set(remoteJid, senderName);
                    store.contacts.set(cleanJid(rawRemote), senderName);
                    store.upsertChat(remoteJid, senderName);
                }
            }

            const quoted = extractQuoted(msg.message, rawRemote);
            const imgMsg = extractImageMessage(msg.message);
            const msgData = {
                id: id,
                fromMe: fromMe,
                text: text,
                timestamp: timestamp
            };
            if (imgMsg) {
                msgData.hasImage = true;
                saveMediaIfImage(msg, sessionDir);
            }
            if (isGroup && !fromMe) {
                if (participant) msgData.participant = participant;
                msgData.senderName = senderName || (participant ? getBestName(participant, '') : '');
            }
            if (quoted) {
                msgData.quotedId = quoted.id;
                msgData.quotedText = quoted.text;
                msgData.quotedSender = quoted.sender;
            }

            store.addMessage(remoteJid, msgData);
            store.save();

            const evMsg = {
                type: 'message',
                device_id: deviceId,
                id: id,
                from: remoteJid,
                name: getBestName(remoteJid, ''),
                fromMe: fromMe,
                text: text,
                timestamp: timestamp
            };
            if (imgMsg) {
                evMsg.hasImage = true;
            }
            if (isGroup && !fromMe) {
                if (participant) evMsg.participant = participant;
                evMsg.senderName = msgData.senderName;
            }
            if (quoted) {
                evMsg.quotedId = quoted.id;
                evMsg.quotedText = quoted.text;
                evMsg.quotedSender = quoted.sender;
            }
            safeSend(evMsg);
        }
    });

    sock.ev.on('call', (calls) => {
        if (!calls || !Array.isArray(calls)) return;
        for (const call of calls) {
            if (!call) continue;
            const remoteJid = normalizeJid(call.from);
            safeSend({
                type: 'call',
                device_id: deviceId,
                callId: call.id,
                from: remoteJid,
                name: getBestName(remoteJid, ''),
                status: call.status,
                isVideo: call.isVideo,
                isGroup: call.isGroup,
                timestamp: (call.date ? new Date(call.date).getTime() : Date.now())
            });
        }
    });

    // Stdin commands
    const rl = readline.createInterface({ input: process.stdin, terminal: false });
    rl.on('line', async (line) => {
        try {
            if (!line.trim()) return;
            const cmd = JSON.parse(line);
            if (cmd.action === 'send_message') {
                let targetJid = cmd.to;
                if (!targetJid.includes('@')) {
                    targetJid = targetJid + '@s.whatsapp.net';
                }
                const options = {};
                if (cmd.quotedId) {
                    options.quoted = {
                        key: {
                            remoteJid: targetJid,
                            id: cmd.quotedId,
                            fromMe: Boolean(cmd.quotedFromMe),
                            participant: cmd.quotedParticipant || undefined
                        },
                        message: {
                            conversation: cmd.quotedText || ''
                        }
                    };
                }
                const sent = await sock.sendMessage(targetJid, { text: cmd.text }, options);
                const sentId = sent?.key?.id || ('sent_' + Date.now());
                const canonJid = normalizeJid(targetJid);
                const now = Date.now();
                const msgData = {
                    id: sentId,
                    fromMe: true,
                    text: cmd.text,
                    timestamp: now
                };
                if (cmd.quotedId) {
                    msgData.quotedId = cmd.quotedId;
                    msgData.quotedText = resolveMentions(cmd.quotedText || '');
                    msgData.quotedSender = cmd.quotedSender ? getBestName(cmd.quotedSender, cmd.quotedSender) : (cmd.quotedFromMe ? 'Você' : getBestName(cmd.quotedParticipant || targetJid, ''));
                }
                store.addMessage(canonJid, msgData);
                store.upsertChat(canonJid, getBestName(canonJid, ''), 0, cmd.text, now);
                store.save();
                safeSend({ type: 'ack', id: cmd.id, status: 'sent', messageId: sentId });
                const evMsg = {
                    type: 'message',
                    device_id: deviceId,
                    id: sentId,
                    from: canonJid,
                    name: getBestName(canonJid, ''),
                    fromMe: true,
                    text: cmd.text,
                    timestamp: now
                };
                if (cmd.quotedId) {
                    evMsg.quotedId = cmd.quotedId;
                    evMsg.quotedText = msgData.quotedText;
                    evMsg.quotedSender = msgData.quotedSender;
                }
                safeSend(evMsg);
            } else if (cmd.action === 'send_audio') {
                let targetJid = cmd.to;
                if (!targetJid.includes('@')) {
                    targetJid = targetJid + '@s.whatsapp.net';
                }
                const audioBuffer = Buffer.from(cmd.base64, 'base64');
                const sent = await sock.sendMessage(targetJid, {
                    audio: audioBuffer,
                    mimetype: cmd.mimetype || 'audio/amr',
                    ptt: true
                });
                const sentId = sent?.key?.id || ('audio_' + Date.now());
                const canonJid = normalizeJid(targetJid);
                const now = Date.now();
                store.addMessage(canonJid, {
                    id: sentId,
                    fromMe: true,
                    text: '[Audio]',
                    timestamp: now
                });
                store.upsertChat(canonJid, getBestName(canonJid, ''), 0, '[Audio]', now);
                store.save();
                safeSend({ type: 'ack', id: cmd.id, status: 'sent', messageId: sentId });
                safeSend({
                    type: 'message',
                    device_id: deviceId,
                    id: sentId,
                    from: canonJid,
                    name: getBestName(canonJid, ''),
                    fromMe: true,
                    text: '[Audio]',
                    timestamp: now
                });
            } else if (cmd.action === 'mark_read') {
                const targetJid = cmd.jid;
                if (targetJid) {
                    const canonJid = normalizeJid(targetJid);
                    const chat = store.chats.get(canonJid) || store.chats.get(targetJid);
                    if (chat) {
                        chat.unread = 0;
                        store.chats.set(canonJid, chat);
                    }
                    if (store.chats.has(targetJid)) {
                        const c2 = store.chats.get(targetJid);
                        c2.unread = 0;
                        store.chats.set(targetJid, c2);
                    }
                    const msgs = store.messages.get(canonJid) || store.messages.get(targetJid);
                    if (msgs && Array.isArray(msgs)) {
                        const unreadKeys = msgs
                            .filter(m => !m.fromMe && m.id)
                            .slice(-15)
                            .map(m => ({
                                remoteJid: targetJid,
                                id: m.id
                            }));
                        if (unreadKeys.length > 0) {
                            try {
                                await sock.readMessages(unreadKeys);
                            } catch (e) {}
                        }
                    }
                    store.save();
                }
            } else if (cmd.action === 'fetch_history') {
                const targetJid = normalizeJid(cmd.jid);
                if (Array.isArray(cmd.missingIds) && cmd.missingIds.length > 0) {
                    for (const mId of cmd.missingIds) {
                        try {
                            if (sock.requestPlaceholderResend) {
                                sock.requestPlaceholderResend({
                                    remoteJid: targetJid,
                                    id: mId,
                                    fromMe: false
                                });
                            }
                        } catch (e) {}
                    }
                }
                const list = store.messages.get(targetJid) || [];
                if (sock.fetchMessageHistory) {
                    try {
                        const cursor = list.length > 0 ? list[list.length - 1] : null;
                        const key = cursor ? {
                            remoteJid: targetJid,
                            id: cursor.id,
                            fromMe: Boolean(cursor.fromMe)
                        } : {
                            remoteJid: targetJid,
                            id: cmd.oldestId || 'dummy',
                            fromMe: false
                        };
                        const ts = cursor?.timestamp || cmd.oldestTs || Date.now();
                        await sock.fetchMessageHistory(50, key, ts);
                        safeSend({ type: 'history_requested', jid: targetJid });
                    } catch (err) {
                        safeSend({ type: 'error', error: err.message });
                    }
                }
            } else if (cmd.action === 'reject_call') {
                if (sock.rejectCall) {
                    await sock.rejectCall(cmd.callId, cmd.from);
                }
            } else if (cmd.action === 'logout') {
                await sock.logout();
            }
        } catch (e) {
            safeSend({ type: 'error', error: e.message });
        }
    });
}

startBridge().catch(err => {
    safeSend({ type: 'error', error: err.message });
    process.exit(1);
});
