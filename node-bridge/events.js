const { DisconnectReason } = require('@whiskeysockets/baileys');
const { safeSend, cleanJid, normalizeJid, resolveMentions, getBestName } = require('./utils');
const { extractText, extractImageMessage, extractQuoted, saveMediaIfImage } = require('./messages');

function registerEvents(sock, { deviceId, sessionDir, store, saveCreds, onReconnect }) {
    sock.ev.on('creds.update', saveCreds);

    sock.ev.on('connection.update', async (update) => {
        const { connection, lastDisconnect, qr } = update;
        if (qr) {
            safeSend({ type: 'qr', device_id: deviceId, qr: qr });
        }
        if (connection === 'close') {
            const shouldReconnect = (lastDisconnect?.error)?.output?.statusCode !== DisconnectReason.loggedOut;
            safeSend({ type: 'connection', device_id: deviceId, status: 'close', reconnect: shouldReconnect });
            if (shouldReconnect && onReconnect) {
                setTimeout(onReconnect, 3000);
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

            // Automatically request history and placeholder resend for groups with missing participants
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
            const targetJid = normalizeJid(cId, store);
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
            const targetJid = normalizeJid(ch.id, store);
            const name = ch.name || getBestName(ch.id, '', store);
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
                const targetJid = normalizeJid(update.id, store);
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
                const targetJid = normalizeJid(ch.id, store);
                const name = ch.name || getBestName(ch.id, '', store);
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
                const text = resolveMentions(rawText, store);

                const remoteJid = normalizeJid(rawRemote, store);
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

                const quoted = extractQuoted(msg.message, rawRemote, store);
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
                    msgData.senderName = senderName || (participant ? getBestName(participant, '', store) : '');
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

            const remoteJid = normalizeJid(rawRemote, store);
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
            const text = resolveMentions(rawText, store);

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

            const quoted = extractQuoted(msg.message, rawRemote, store);
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
                msgData.senderName = senderName || (participant ? getBestName(participant, '', store) : '');
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
                name: getBestName(remoteJid, '', store),
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
            const remoteJid = normalizeJid(call.from, store);
            safeSend({
                type: 'call',
                device_id: deviceId,
                callId: call.id,
                from: remoteJid,
                name: getBestName(remoteJid, '', store),
                status: call.status,
                isVideo: call.isVideo,
                isGroup: call.isGroup,
                timestamp: (call.date ? new Date(call.date).getTime() : Date.now())
            });
        }
    });
}

module.exports = { registerEvents };
