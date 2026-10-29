// Build inputs: sha256:49fe3e5cc6842899c8a6a17797e39e77e17f879d38e27ac3724f45a4687d9087
const url = new URL("shallot_audio.wasm", import.meta.url);
export default async function loadAudioWasm() {
    const response = await fetch(url);
    return response.arrayBuffer();
}
