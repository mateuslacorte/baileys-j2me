const readline = require('readline');
const { safeSend, normalizeJid, resolveMentions, getBestName } = require('./utils');

function registerCommands(sock, { deviceId, store }) {
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
                const canonJid = normalizeJid(targetJid, store);
                const now = Date.now();
                const msgData = {
                    id: sentId,
                    fromMe: true,
                    text: cmd.text,
                    timestamp: now
                };
                if (cmd.quotedId) {
                    msgData.quotedId = cmd.quotedId;
                    msgData.quotedText = resolveMentions(cmd.quotedText || '', store);
                    msgData.quotedSender = cmd.quotedSender ? getBestName(cmd.quotedSender, cmd.quotedSender, store) : (cmd.quotedFromMe ? 'Você' : getBestName(cmd.quotedParticipant || targetJid, '', store));
                }
                store.addMessage(canonJid, msgData);
                store.upsertChat(canonJid, getBestName(canonJid, '', store), 0, cmd.text, now);
                store.save();

                safeSend({ type: 'ack', id: cmd.id, status: 'sent', messageId: sentId });
                const evMsg = {
                    type: 'message',
                    device_id: deviceId,
                    id: sentId,
                    from: canonJid,
                    name: getBestName(canonJid, '', store),
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
                const canonJid = normalizeJid(targetJid, store);
                const now = Date.now();
                store.addMessage(canonJid, {
                    id: sentId,
                    fromMe: true,
                    text: '[Audio]',
                    timestamp: now
                });
                store.upsertChat(canonJid, getBestName(canonJid, '', store), 0, '[Audio]', now);
                store.save();

                safeSend({ type: 'ack', id: cmd.id, status: 'sent', messageId: sentId });
                safeSend({
                    type: 'message',
                    device_id: deviceId,
                    id: sentId,
                    from: canonJid,
                    name: getBestName(canonJid, '', store),
                    fromMe: true,
                    text: '[Audio]',
                    timestamp: now
                });
            } else if (cmd.action === 'mark_read') {
                const targetJid = cmd.jid;
                if (targetJid) {
                    const canonJid = normalizeJid(targetJid, store);
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
                const targetJid = normalizeJid(cmd.jid, store);
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

    return rl;
}

module.exports = { registerCommands };
