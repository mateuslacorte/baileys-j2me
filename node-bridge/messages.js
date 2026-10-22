const fs = require('fs');
const path = require('path');
const pino = require('pino');
const { downloadMediaMessage } = require('@whiskeysockets/baileys');
const { normalizeJid, resolveMentions, getBestName } = require('./utils');

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

function extractImageMessage(message) {
    if (!message) return null;
    if (message.ephemeralMessage?.message) return extractImageMessage(message.ephemeralMessage.message);
    if (message.viewOnceMessage?.message) return extractImageMessage(message.viewOnceMessage.message);
    if (message.viewOnceMessageV2?.message) return extractImageMessage(message.viewOnceMessageV2.message);
    if (message.documentWithCaptionMessage?.message) return extractImageMessage(message.documentWithCaptionMessage.message);
    return message.imageMessage || null;
}

function extractQuoted(message, remoteJid, store) {
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
    let quotedText = resolveMentions(extractText(qMsg), store) || '';
    let sender = ctx.participant ? normalizeJid(ctx.participant, store) : '';
    let senderName = sender ? getBestName(sender, '', store) : '';

    if (!quotedText && ctx.stanzaId && store) {
        const chatMsgs = store.messages.get(normalizeJid(remoteJid || '', store));
        if (chatMsgs) {
            const found = chatMsgs.find(m => m.id === ctx.stanzaId);
            if (found) {
                quotedText = resolveMentions(found.text, store);
                if (!senderName) {
                    senderName = found.fromMe ? 'Você' : (found.senderName || getBestName(found.participant || '', '', store));
                }
            }
        }
    }

    if (!senderName && remoteJid) {
        senderName = getBestName(normalizeJid(remoteJid, store), '', store);
    }

    return {
        id: ctx.stanzaId,
        sender: senderName || 'Mensagem',
        text: quotedText
    };
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

module.exports = {
    extractText,
    extractImageMessage,
    extractQuoted,
    saveMediaIfImage
};
