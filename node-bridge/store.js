const fs = require('fs');
const path = require('path');

class FileChatStore {
    constructor(dir) {
        this.sessionDir = dir;
        this.filePath = path.join(dir, 'chats_store.json');
        this.chats = new Map(); // jid -> { jid, name, lastMessage, timestamp, unread }
        this.messages = new Map(); // jid -> [ { id, fromMe, text, timestamp } ]
        this.contacts = new Map(); // jid -> name
        this.lidMap = new Map(); // lid -> phone
        this.phoneToLid = new Map(); // phone -> lid
        this.loadLidMappings();
        this.load();
    }

    loadLidMappings() {
        try {
            if (!fs.existsSync(this.sessionDir)) return;
            const files = fs.readdirSync(this.sessionDir);
            for (const f of files) {
                if (f.startsWith('lid-mapping-')) {
                    if (f.endsWith('_reverse.json')) {
                        const lidNum = f.replace('lid-mapping-', '').replace('_reverse.json', '');
                        try {
                            const phone = JSON.parse(fs.readFileSync(path.join(this.sessionDir, f), 'utf8'));
                            if (phone && lidNum) {
                                const lidJid = lidNum + '@lid';
                                const phoneJid = phone + '@s.whatsapp.net';
                                this.lidMap.set(lidJid, phoneJid);
                                this.phoneToLid.set(phoneJid, lidJid);
                            }
                        } catch (e) {}
                    } else if (f.endsWith('.json')) {
                        const phoneNum = f.replace('lid-mapping-', '').replace('.json', '');
                        try {
                            const lid = JSON.parse(fs.readFileSync(path.join(this.sessionDir, f), 'utf8'));
                            if (lid && phoneNum) {
                                const lidJid = lid + '@lid';
                                const phoneJid = phoneNum + '@s.whatsapp.net';
                                this.lidMap.set(lidJid, phoneJid);
                                this.phoneToLid.set(phoneJid, lidJid);
                            }
                        } catch (e) {}
                    }
                }
            }
        } catch (e) {}
    }

    canonicalJid(jid) {
        if (!jid) return '';
        let clean = jid.split(':')[0];
        if (clean.includes('@')) {
            const at = clean.indexOf('@');
            clean = clean.substring(0, at) + clean.substring(at);
        }
        if (clean.endsWith('@lid') && this.lidMap.has(clean)) {
            return this.lidMap.get(clean);
        }
        return clean;
    }

    load() {
        try {
            if (fs.existsSync(this.filePath)) {
                const data = JSON.parse(fs.readFileSync(this.filePath, 'utf8'));
                if (data.contacts) {
                    for (const [k, v] of Object.entries(data.contacts)) {
                        this.contacts.set(this.canonicalJid(k), v);
                    }
                }
                if (data.chats) {
                    for (const c of data.chats) {
                        const cJid = this.canonicalJid(c.jid);
                        this.upsertChat(cJid, c.name, c.unread || 0, c.lastMessage, c.timestamp);
                    }
                }
                if (data.messages) {
                    for (const [k, v] of Object.entries(data.messages)) {
                        const cJid = this.canonicalJid(k);
                        if (Array.isArray(v)) {
                            for (const m of v) {
                                this.addMessage(cJid, m);
                            }
                        }
                    }
                }
            }
        } catch (e) {}
    }

    save() {
        try {
            const data = {
                chats: Array.from(this.chats.values()),
                messages: Object.fromEntries(this.messages),
                contacts: Object.fromEntries(this.contacts),
                lidMap: Object.fromEntries(this.lidMap)
            };
            fs.writeFileSync(this.filePath, JSON.stringify(data, null, 2), 'utf8');
        } catch (e) {}
    }

    upsertChat(rawJid, name, unread = 0, lastMessage = '', timestamp = 0) {
        const jid = this.canonicalJid(rawJid);
        if (!jid) return;
        const existing = this.chats.get(jid) || { jid, name: '', lastMessage: '', timestamp: 0, unread: 0 };
        if (name && (!existing.name || /^\d+$/.test(existing.name) || existing.name === jid.split('@')[0])) {
            existing.name = name;
        }
        if (unread) existing.unread = Math.max(existing.unread || 0, unread);
        if (lastMessage && (!existing.lastMessage || timestamp >= (existing.timestamp || 0))) {
            existing.lastMessage = lastMessage;
        }
        if (timestamp && timestamp > (existing.timestamp || 0)) {
            existing.timestamp = timestamp;
        }
        this.chats.set(jid, existing);
    }

    addMessage(rawJid, msg) {
        const jid = this.canonicalJid(rawJid);
        if (!jid || !msg) return;
        let list = this.messages.get(jid);
        if (!list) {
            list = [];
            this.messages.set(jid, list);
        }

        // Deduplicate by ID and content+timestamp
        let existingMsg = list.find(m =>
            (msg.id && m.id === msg.id) ||
            (m.fromMe === msg.fromMe && m.text === msg.text && Math.abs((m.timestamp || 0) - (msg.timestamp || 0)) < 15000)
        );

        if (!existingMsg) {
            const entry = {
                id: msg.id,
                fromMe: Boolean(msg.fromMe),
                text: msg.text || '',
                timestamp: msg.timestamp || Date.now()
            };
            if (msg.hasImage) entry.hasImage = true;
            if (msg.participant) entry.participant = msg.participant;
            if (msg.senderName) entry.senderName = msg.senderName;
            if (msg.quotedId) entry.quotedId = msg.quotedId;
            if (msg.quotedText) entry.quotedText = msg.quotedText;
            if (msg.quotedSender) entry.quotedSender = msg.quotedSender;
            list.push(entry);

            // Keep strictly sorted chronologically (oldest to newest)
            list.sort((a, b) => (a.timestamp || 0) - (b.timestamp || 0));

            if (list.length > 50) list.shift();
        } else {
            // Merge metadata if available
            if (msg.hasImage && !existingMsg.hasImage) existingMsg.hasImage = true;
            if (msg.participant && !existingMsg.participant) existingMsg.participant = msg.participant;
            if (msg.senderName && !existingMsg.senderName) existingMsg.senderName = msg.senderName;
            if (msg.quotedId && !existingMsg.quotedId) existingMsg.quotedId = msg.quotedId;
            if (msg.quotedText && !existingMsg.quotedText) existingMsg.quotedText = msg.quotedText;
            if (msg.quotedSender && !existingMsg.quotedSender) existingMsg.quotedSender = msg.quotedSender;
        }

        const chat = this.chats.get(jid) || { jid, name: jid.split('@')[0], lastMessage: '', timestamp: 0, unread: 0 };
        const msgTs = msg.timestamp || Date.now();
        if (msgTs >= (chat.timestamp || 0)) {
            chat.lastMessage = msg.text;
            chat.timestamp = msgTs;
        }
        this.chats.set(jid, chat);
    }

    getSnapshot(limit = 40) {
        const sortedChats = Array.from(this.chats.values())
            .filter(c => (c.lastMessage && c.lastMessage.trim().length > 0) || (this.messages.get(c.jid) && this.messages.get(c.jid).length > 0))
            .sort((a, b) => (b.timestamp || 0) - (a.timestamp || 0))
            .slice(0, limit);
        return {
            chats: sortedChats,
            messages: []
        };
    }
}

module.exports = FileChatStore;
