const ICE_SERVERS = [{ urls: "stun:stun.l.google.com:19302" }];

export async function postJson(url, body, headers = {}) {
  const response = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json", ...headers },
    body: JSON.stringify(body),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data.error || `Request failed (${response.status})`);
  return data;
}

export async function fetchHandoffRoom(line, sessionId) {
  const response = await fetch(`/api/handoff/${encodeURIComponent(line)}/${encodeURIComponent(sessionId)}`, { cache: "no-store" });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data.error || "Handoff room is unavailable");
  return data;
}

export function callerPreviewFromTurns(turns = []) {
  const userTurns = turns.filter((turn) => turn.role === "user" || turn.role === "caller");
  const text = userTurns.map((turn) => turn.message || turn.text || "").join(" ").replace(/\s+/g, " ").trim();
  const nameStops = new Set(["and", "from", "about", "calling", "would", "like", "here", "with", "for", "the", "a", "an", "i", "it", "its"]);
  const title = (part) => part.replace(/\b\w/g, (letter) => letter.toUpperCase());
  const nameMatch = text.match(/(?:it['’]?s|i(?:['’]m| am)|my name is|this is)\s+([A-Za-z][A-Za-z'-]+)(?:\s+([A-Za-z][A-Za-z'-]+))?/i);
  let firstName = nameMatch?.[1] || "";
  let lastName = nameMatch?.[2] && !nameStops.has(nameMatch[2].toLowerCase()) ? nameMatch[2] : "";
  if (!firstName) {
    const fromName = text.match(/\b([A-Za-z][A-Za-z'-]+)\s+from\s+[A-Za-z]/i);
    if (fromName && !nameStops.has(fromName[1].toLowerCase())) firstName = fromName[1];
  }
  const callerName = [firstName, lastName].filter(Boolean).map(title).join(" ");
  const companyMatch = text.match(/(?:from|company(?:\s+is)?|calling (?:from|on behalf of)|i work (?:at|for))\s+([A-Za-z][^,.]{1,50}?)(?:\s+business)?(?=\s+(?:about|regarding|calling|and|i['’]m|i am)|\s*[.,]|$)/i);
  const claimedCompany = companyMatch ? companyMatch[1].trim().replace(/\s+/g, " ") : "";
  const labeledReference = text.match(/\b(?:ref(?:erence)?|po|p\.?o\.?|invoice|ticket|account)\s*(?:number|no\.?|#|is|:)?\s*([A-Za-z0-9][A-Za-z0-9/-]{1,})\b/i);
  const dottedReference = text.match(/\b([A-Z]{1,5}-\d{2,})\b/);
  const rawReference = labeledReference?.[1] || dottedReference?.[1] || "";
  const reference = /[0-9]/.test(rawReference) ? rawReference : "";
  return {
    callerName,
    claimedCompany,
    reference,
    requestedAction: text.slice(0, 180),
  };
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

const MAX_RECORD_MS = 10_000;
const MAX_IDLE_MS = 4_000;
const MIN_SPEECH_MS = 320;
const END_SILENCE_MS = 720;
const MIN_AUDIO_BYTES = 1200;
const SPEECH_RMS = 0.018;

function recorderMime() {
  const types = ["audio/webm;codecs=opus", "audio/webm", "audio/mp4"];
  return types.find((type) => typeof MediaRecorder !== "undefined" && MediaRecorder.isTypeSupported(type)) || "";
}

function streamCaptions({ stream, role, line, sessionId, captionKey, enqueueUpload, onText, isClosed, reviveStream }) {
  const mime = recorderMime();
  if (!mime || !stream) return speechRecognizer(role, onText);
  let active = true;
  let captureStream = stream;
  const AudioContext = window.AudioContext || window.webkitAudioContext;
  let audioContext = null;
  let analyser = null;
  let samples = null;

  function attachAnalyser(nextStream) {
    if (!AudioContext || !nextStream) return;
    try {
      audioContext?.close?.().catch(() => {});
      audioContext = new AudioContext();
      audioContext.resume?.().catch(() => {});
      const source = audioContext.createMediaStreamSource(nextStream);
      analyser = audioContext.createAnalyser();
      analyser.fftSize = 1024;
      analyser.smoothingTimeConstant = 0.2;
      samples = new Float32Array(analyser.fftSize);
      source.connect(analyser);
    } catch {
      audioContext?.close?.().catch(() => {});
      audioContext = null;
      analyser = null;
      samples = null;
    }
  }
  attachAnalyser(captureStream);

  function speechLevel() {
    if (!analyser || !samples) return 1;
    analyser.getFloatTimeDomainData(samples);
    let sum = 0;
    for (const sample of samples) sum += sample * sample;
    return Math.sqrt(sum / samples.length);
  }

  async function recordOnce() {
    const parts = [];
    let recorder;
    try {
      recorder = new MediaRecorder(captureStream, { mimeType: mime, audioBitsPerSecond: 24_000 });
    } catch {
      try {
        recorder = new MediaRecorder(captureStream, { mimeType: mime });
      } catch {
        return { blob: new Blob([], { type: mime }), speechDetected: false };
      }
    }
    recorder.ondataavailable = (event) => {
      if (event.data?.size) parts.push(event.data);
    };
    return new Promise((resolve) => {
      let firstSpeechAt = 0;
      let lastSpeechAt = 0;
      let monitor = null;
      const finish = () => {
        clearInterval(monitor);
        resolve({ blob: new Blob(parts, { type: mime }), speechDetected: Boolean(firstSpeechAt) });
      };
      recorder.onerror = finish;
      recorder.onstop = finish;
      try {
        recorder.start();
      } catch {
        finish();
        return;
      }
      const startedAt = Date.now();
      monitor = setInterval(() => {
        const now = Date.now();
        if (speechLevel() >= SPEECH_RMS) {
          firstSpeechAt ||= now;
          lastSpeechAt = now;
        }
        const enoughSpeech = firstSpeechAt && lastSpeechAt - firstSpeechAt >= MIN_SPEECH_MS;
        const utteranceEnded = enoughSpeech && now - lastSpeechAt >= END_SILENCE_MS;
        const idleExpired = !firstSpeechAt && now - startedAt >= MAX_IDLE_MS;
        const recordExpired = now - startedAt >= MAX_RECORD_MS;
        if ((utteranceEnded || idleExpired || recordExpired) && recorder.state === "recording") {
          clearInterval(monitor);
          try { recorder.stop(); } catch { finish(); }
        }
      }, 80);
      recorder.addEventListener("stop", () => clearInterval(monitor), { once: true });
    });
  }

  async function upload(blob) {
    try {
      const response = await fetch(`/api/handoff/${encodeURIComponent(line)}/transcribe`, {
        method: "POST",
        headers: {
          "content-type": blob.type || mime,
          "x-session-id": sessionId,
          "x-role": role,
          "x-caption-key": captionKey,
        },
        body: blob,
      });
      const data = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(data.error || `Caption request failed (${response.status})`);
      const text = String(data.text || "").trim();
      if (text) onText({ role, text });
    } catch (error) {
      console.warn("[handoff] caption request failed", error);
    }
  }

  async function cycle() {
    while (active && !isClosed?.()) {
      const tracks = captureStream.getAudioTracks();
      if (!tracks.length || tracks.every((track) => track.readyState !== "live")) {
        if (typeof reviveStream === "function") {
          try {
            const next = await reviveStream();
            if (next) {
              captureStream = next;
              attachAnalyser(captureStream);
            }
          } catch {}
        }
        await sleep(400);
        continue;
      }
      const { blob, speechDetected } = await recordOnce();
      if (!active || isClosed?.()) return;
      if (!speechDetected || blob.size < MIN_AUDIO_BYTES) continue;
      // Keep recording while the previous utterance is transcribed. Serializing
      // only the uploads preserves speaker order without dropping live audio.
      enqueueUpload(() => upload(blob));
    }
  }

  cycle();
  return {
    available: true,
    start() {},
    stop() {
      active = false;
      audioContext?.close?.().catch(() => {});
    },
  };
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
  constructor({ role, line, sessionId, remoteAudio, onState, onTranscript, captureCaptions = role === "caller" }) {
    this.role = role;
    this.line = line;
    this.sessionId = sessionId;
    this.remoteAudio = remoteAudio;
    this.onState = onState || (() => {});
    this.onTranscript = onTranscript || (() => {});
    this.captureCaptions = captureCaptions;
    this.captionKey = crypto.randomUUID?.() || `${Date.now()}-${Math.random().toString(16).slice(2)}`;
    this.pc = null;
    this.localStream = null;
    this.captions = [];
    this.captionQueue = Promise.resolve();
    this.localCaptioned = false;
    this.remoteCaptioned = false;
    this.remoteSet = false;
    this.pendingIce = [];
    this.localIce = [];
    this.roomReady = role === "staff";
    this.muted = false;
    this.closed = false;
    this.startedAt = 0;
    this.disconnectTimer = null;
  }

  async startCaller({ disclosureGiven = true, callerName = "", requestedAction = "" } = {}) {
    await this.#openPeer();
    const offer = await this.pc.createOffer();
    await this.pc.setLocalDescription(offer);
    await postJson(`/api/handoff/${encodeURIComponent(this.line)}/ring`, {
      session_id: this.sessionId,
      disclosure_given: disclosureGiven,
      caller_name: callerName || null,
      requested_action: requestedAction || null,
      caption_key: this.captionKey,
      offer: { type: offer.type, sdp: offer.sdp },
    });
    this.roomReady = true;
    for (const candidate of this.localIce.splice(0)) this.#sendIce(candidate);
    this.startedAt = Date.now();
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
    await postJson(`/api/handoff/${encodeURIComponent(this.line)}/signal`, {
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
      this.#listen();
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

  async reviveLocalAudio() {
    if (this.closed) return this.localStream;
    const live = this.localStream?.getAudioTracks().some((track) => track.readyState === "live");
    if (live) return this.localStream;
    this.localStream = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: true, noiseSuppression: true },
      video: false,
    });
    const track = this.localStream.getAudioTracks()[0];
    const sender = this.pc?.getSenders().find((item) => item.track?.kind === "audio" || item.track == null);
    if (track && sender) {
      try { await sender.replaceTrack(track); } catch {}
    } else if (track && this.pc) {
      this.pc.addTrack(track, this.localStream);
    }
    if (this.muted) this.setMuted(true);
    return this.localStream;
  }

  async hangup() {
    if (this.closed) return;
    try {
      await postJson(`/api/handoff/${encodeURIComponent(this.line)}/hangup`, { session_id: this.sessionId, from: this.role });
    } catch {}
    this.close(false);
    this.onState("ended");
  }

  close(notify = true) {
    if (this.closed) return;
    this.closed = true;
    clearTimeout(this.disconnectTimer);
    this.disconnectTimer = null;
    for (const caption of this.captions) caption.stop();
    this.captions = [];
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
      const candidate = event.candidate.toJSON();
      if (!this.roomReady) {
        this.localIce.push(candidate);
        return;
      }
      this.#sendIce(candidate);
    };
    this.pc.ontrack = (event) => {
      const stream = event.streams[0] || new MediaStream(event.track ? [event.track] : []);
      if (this.remoteAudio) {
        this.remoteAudio.srcObject = stream;
        this.remoteAudio.play?.().catch(() => {});
      }
      if (this.captureCaptions && !this.remoteCaptioned && stream.getAudioTracks().length) {
        this.remoteCaptioned = true;
        this.#captionStream(stream, this.role === "staff" ? "caller" : "staff");
      }
    };
    this.pc.onconnectionstatechange = () => {
      const state = this.pc?.connectionState;
      if (state === "connected") {
        clearTimeout(this.disconnectTimer);
        this.disconnectTimer = null;
        this.onState("connected");
      }
      if (state === "failed") {
        this.#endAfterConnectionLoss("failed");
      } else if (state === "disconnected" && !this.disconnectTimer) {
        this.disconnectTimer = setTimeout(() => this.#endAfterConnectionLoss("disconnected"), 4_000);
      }
    };
    this.localStream = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: true, noiseSuppression: true },
      video: false,
    });
    for (const track of this.localStream.getTracks()) this.pc.addTrack(track, this.localStream);
  }

  #sendIce(candidate) {
    postJson(`/api/handoff/${encodeURIComponent(this.line)}/signal`, {
      session_id: this.sessionId,
      from: this.role,
      kind: "ice",
      payload: candidate,
    }).catch(() => {});
  }

  async #listen() {
    if (!this.captureCaptions || this.localCaptioned || this.closed) return;
    this.localCaptioned = true;
    try { await this.reviveLocalAudio(); } catch {}
    this.#captionStream(this.localStream, this.role);
  }

  #captionStream(stream, role) {
    if (!stream || this.closed) return;
    const caption = streamCaptions({
      stream,
      role,
      line: this.line,
      sessionId: this.sessionId,
      captionKey: this.captionKey,
      reviveStream: role === this.role ? () => this.reviveLocalAudio() : null,
      enqueueUpload: (task) => {
        this.captionQueue = this.captionQueue.then(task);
      },
      isClosed: () => this.closed,
      onText: ({ text }) => {
        this.onTranscript({ role, text });
        if (!recorderMime()) {
          postJson(`/api/handoff/${encodeURIComponent(this.line)}/transcript`, {
            session_id: this.sessionId,
            role,
            text,
          }, { "x-caption-key": this.captionKey }).catch(() => {});
        }
      },
    });
    caption.start();
    this.captions.push(caption);
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
        const room = await fetchHandoffRoom(this.line, this.sessionId);
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
    if (!this.remoteSet && !this.closed) {
      try {
        await postJson(`/api/handoff/${encodeURIComponent(this.line)}/hangup`, { session_id: this.sessionId, from: this.role });
      } catch {}
      this.close(false);
      this.onState("timeout");
    }
  }

  async #endAfterConnectionLoss(reason) {
    if (this.closed) return;
    try {
      await postJson(`/api/handoff/${encodeURIComponent(this.line)}/hangup`, { session_id: this.sessionId, from: this.role });
    } catch {}
    this.close(false);
    this.onState(reason);
  }
}
