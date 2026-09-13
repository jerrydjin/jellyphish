const ICE_SERVERS = [{ urls: "stun:stun.l.google.com:19302" }];

export async function postJson(url, body) {
  const response = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data.error || `Request failed (${response.status})`);
  return data;
}

export async function fetchHandoffRoom(sessionId) {
  const response = await fetch(`/api/handoff/${encodeURIComponent(sessionId)}`, { cache: "no-store" });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data.error || "Handoff room is unavailable");
  return data;
}

export function callerPreviewFromTurns(turns = []) {
  const userTurns = turns.filter((turn) => turn.role === "user" || turn.role === "caller");
  const text = userTurns.map((turn) => turn.message || turn.text || "").join(" ").replace(/\s+/g, " ").trim();
  const nameMatch = text.match(/(?:i(?:['’]m| am)|my name is|this is)\s+([A-Z][A-Za-z'-]+)/);
  return {
    callerName: nameMatch ? nameMatch[1] : "",
    requestedAction: text.slice(0, 180),
  };
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function speechRecognizer(role, onFinal) {
  const SpeechRecognition = window.SpeechRecognition || window.webkitSpeechRecognition;
  if (!SpeechRecognition) return { available: false, start() {}, stop() {} };
  const recognition = new SpeechRecognition();
  recognition.continuous = true;
  recognition.interimResults = true;
  recognition.lang = "en-AU";
  let active = false;
  recognition.onresult = (event) => {
    for (let index = event.resultIndex; index < event.results.length; index += 1) {
      const result = event.results[index];
      if (!result.isFinal) continue;
      const text = String(result[0]?.transcript || "").trim();
      if (text) onFinal({ role, text });
    }
  };
  recognition.onend = () => {
    if (active) {
      try { recognition.start(); } catch {}
    }
  };
  return {
    available: true,
    start() {
      active = true;
      try { recognition.start(); } catch {}
    },
    stop() {
      active = false;
      try { recognition.stop(); } catch {}
    },
  };
}

export class HandoffCall {
  constructor({ role, sessionId, remoteAudio, onState }) {
    this.role = role;
    this.sessionId = sessionId;
    this.remoteAudio = remoteAudio;
    this.onState = onState || (() => {});
    this.pc = null;
    this.localStream = null;
    this.speech = null;
    this.remoteSet = false;
    this.pendingIce = [];
    this.muted = false;
    this.closed = false;
    this.startedAt = 0;
  }

  async startCaller({ disclosureGiven = true, callerName = "", requestedAction = "" } = {}) {
    await this.#openPeer();
    const offer = await this.pc.createOffer();
    await this.pc.setLocalDescription(offer);
    await postJson("/api/handoff/ring", {
      session_id: this.sessionId,
      disclosure_given: disclosureGiven,
      caller_name: callerName || null,
      requested_action: requestedAction || null,
      offer: { type: offer.type, sdp: offer.sdp },
    });
    this.startedAt = Date.now();
    this.#listen();
    this.onState("ringing");
    this.#waitForAnswer();
    return { status: "ringing" };
  }

  async acceptStaff(room) {
    await this.#openPeer();
    if (!room?.offer) throw new Error("Incoming call is missing an audio offer");
    await this.pc.setRemoteDescription(room.offer);
    this.remoteSet = true;
    await this.#flushIce();
    for (const candidate of room.callerIce || []) {
      try { await this.pc.addIceCandidate(candidate); } catch {}
    }
    const answer = await this.pc.createAnswer();
    await this.pc.setLocalDescription(answer);
    await postJson("/api/handoff/signal", {
      session_id: this.sessionId,
      from: "staff",
      kind: "answer",
      payload: { type: answer.type, sdp: answer.sdp },
    });
    this.startedAt = Date.now();
    this.#listen();
    this.onState("connected");
  }

  async handleSignal(event) {
    if (!event || event.sessionId !== this.sessionId || this.closed) return;
    if (event.kind === "answer" && this.role === "caller" && !this.remoteSet && event.payload) {
      await this.pc.setRemoteDescription(event.payload);
      this.remoteSet = true;
      await this.#flushIce();
      this.onState("connected");
    }
    if (event.kind === "ice" && event.from !== this.role && event.payload) {
      if (!this.remoteSet) this.pendingIce.push(event.payload);
      else {
        try { await this.pc.addIceCandidate(event.payload); } catch {}
      }
    }
    if (event.kind === "hangup" || event.room?.status === "ended") {
      this.onState("ended");
      this.close(false);
    }
  }

  setMuted(muted) {
    this.muted = Boolean(muted);
    for (const track of this.localStream?.getAudioTracks() || []) track.enabled = !this.muted;
  }

  async hangup() {
    if (this.closed) return;
    try {
      await postJson("/api/handoff/hangup", { session_id: this.sessionId, from: this.role });
    } catch {}
    this.close(false);
    this.onState("ended");
  }

  close(notify = true) {
    if (this.closed) return;
    this.closed = true;
    this.speech?.stop();
    this.speech = null;
    for (const track of this.localStream?.getTracks() || []) track.stop();
    this.localStream = null;
    try { this.pc?.close(); } catch {}
    this.pc = null;
    if (this.remoteAudio) this.remoteAudio.srcObject = null;
    if (notify) this.onState("ended");
  }

  async #openPeer() {
    this.pc = new RTCPeerConnection({ iceServers: ICE_SERVERS });
    this.pc.onicecandidate = (event) => {
      if (!event.candidate || this.closed) return;
      postJson("/api/handoff/signal", {
        session_id: this.sessionId,
        from: this.role,
        kind: "ice",
        payload: event.candidate.toJSON(),
      }).catch(() => {});
    };
    this.pc.ontrack = (event) => {
      if (!this.remoteAudio) return;
      this.remoteAudio.srcObject = event.streams[0] || new MediaStream(event.track ? [event.track] : []);
      this.remoteAudio.play?.().catch(() => {});
    };
    this.pc.onconnectionstatechange = () => {
      if (this.pc?.connectionState === "connected") this.onState("connected");
      if (this.pc?.connectionState === "failed" || this.pc?.connectionState === "disconnected") {
        this.onState(this.pc.connectionState);
      }
    };
    this.localStream = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: true, noiseSuppression: true },
      video: false,
    });
    for (const track of this.localStream.getTracks()) this.pc.addTrack(track, this.localStream);
  }

  #listen() {
    this.speech = speechRecognizer(this.role, ({ text }) => {
      postJson("/api/handoff/transcript", {
        session_id: this.sessionId,
        role: this.role,
        text,
      }).catch(() => {});
    });
    this.speech.start();
  }

  async #flushIce() {
    const queued = this.pendingIce.splice(0);
    for (const candidate of queued) {
      try { await this.pc.addIceCandidate(candidate); } catch {}
    }
  }

  async #waitForAnswer() {
    const deadline = Date.now() + 45_000;
    while (!this.closed && !this.remoteSet && Date.now() < deadline) {
      try {
        const room = await fetchHandoffRoom(this.sessionId);
        if (room.answer) {
          await this.handleSignal({ sessionId: this.sessionId, kind: "answer", payload: room.answer, from: "staff" });
          for (const candidate of room.staffIce || []) {
            await this.handleSignal({ sessionId: this.sessionId, kind: "ice", payload: candidate, from: "staff" });
          }
          return;
        }
        if (room.status === "ended") {
          this.onState("ended");
          this.close(false);
          return;
        }
      } catch {}
      await sleep(400);
    }
    if (!this.remoteSet && !this.closed) this.onState("timeout");
  }
}
