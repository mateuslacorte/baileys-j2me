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

function normalizeJid(jid, store) {
    const cleaned = cleanJid(jid);
    return store ? store.canonicalJid(cleaned) : cleaned;
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

function resolveMentions(text, store) {
    if (!text || typeof text !== 'string') return text || '';
    if (!text.includes('@')) return text;
    return text.replace(/@(\d{7,25})/g, (match, digits) => {
        const lidJid = digits + '@lid';
        let phoneJid = null;
        if (store && store.lidMap) {
            phoneJid = store.lidMap.get ? store.lidMap.get(lidJid) : store.lidMap[lidJid];
        }
        let phone = phoneJid ? phoneJid.split('@')[0] : (digits.length <= 13 && !digits.startsWith('2') ? digits : null);
        let contact = null;
        if (store && store.contacts) {
            if (phoneJid && store.contacts.has(phoneJid)) contact = store.contacts.get(phoneJid);
            if (!contact && phone && store.contacts.has(phone)) contact = store.contacts.get(phone);
            if (!contact && store.contacts.has(digits + '@s.whatsapp.net')) contact = store.contacts.get(digits + '@s.whatsapp.net');
            if (!contact && store.contacts.has(digits)) contact = store.contacts.get(digits);
        }

        if (contact && /[a-zA-Z]/.test(contact) && contact !== '~') {
            return '@' + contact.trim();
        }
        if (phone) {
            return '@' + formatPhoneNumber(phone);
        }
        return '@Participante';
    });
}

function getBestName(rawJid, fallback, store) {
    if (!rawJid) return fallback || '';
    const nJid = normalizeJid(rawJid, store);
    if (store && store.contacts) {
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
    }
    if (fallback && fallback.trim().length > 0 && !fallback.trim().endsWith('@lid') && !/^\d{14,}$/.test(fallback.trim())) {
        return fallback.trim();
    }
    const user = nJid.split('@')[0];
    if (nJid.endsWith('@lid') || (user.length >= 14 && user.startsWith('2'))) {
        return 'Participante';
    }
    if (/^\d{7,13}$/.test(user)) {
        return formatPhoneNumber(user);
    }
    return user;
}

module.exports = {
    safeSend,
    cleanJid,
    normalizeJid,
    formatPhoneNumber,
    resolveMentions,
    getBestName
};
