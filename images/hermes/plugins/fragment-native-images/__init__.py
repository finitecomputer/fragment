"""Managed auxiliary vision is a fallback, and every API copy has bounded images."""
from pathlib import Path

# Four 256 KiB image URLs leave room within the route's 6 MiB body cap.
# Canonical history and original files remain untouched.
IMAGES_MAX = 4
IMAGE_BYTES_MAX = 256 * 1024
IMAGE_EDGE_MAX = 1568


def register(ctx):
    from agent import image_routing
    original = image_routing.decide_image_input_mode
    if not getattr(original, "_fragment_native", False):
        def mode(provider, model, cfg, **kwargs):
            # Hermes treats any explicit auxiliary backend as a forced
            # text route. Only its decision gets this copy: the actual
            # backend remains configured for text-only/unknown models.
            if isinstance(cfg, dict):
                auxiliary = cfg.get("auxiliary")
                cfg = {**cfg, "auxiliary": {**(auxiliary if isinstance(auxiliary, dict) else {}), "vision": {}}}
            return original(provider, model, cfg, **kwargs)
        mode._fragment_native = True
        image_routing.decide_image_input_mode = mode
    ctx.register_middleware("llm_request", bound_images)


def shrink(url):
    if not url.startswith("data:"):
        return url, None  # Remote references carry no embedded bytes.
    from agent.vision_message_prep import VisionMessagePrepMixin
    from tools.vision_tools import _build_scale_note, _resize_image_for_vision
    path, cleanup = VisionMessagePrepMixin._materialize_data_url_for_vision(url)
    if not path:
        return None, None
    try:
        from PIL import Image
        with Image.open(path) as image:
            alpha = image.format == "PNG" and (image.mode in {"RGBA", "LA"} or "transparency" in image.info)
            image.verify()
        scale = {}
        resized = _resize_image_for_vision(
            Path(path), max_base64_bytes=IMAGE_BYTES_MAX,
            max_dimension=IMAGE_EDGE_MAX, force_jpeg=not alpha, scale_out=scale)
        return (resized, _build_scale_note(scale, None)) if len(resized) <= IMAGE_BYTES_MAX else (None, None)
    finally:
        if cleanup is not None:
            cleanup.unlink(missing_ok=True)


def bound_images(request=None, **kwargs):
    if not isinstance(request, dict) or not isinstance(request.get("messages"), list):
        return None
    # SDK payload copies can fall back to shallow copies for client/callback
    # objects. Own both list levels we rewrite, even on that path.
    request = {**request, "messages": [
        {**m, "content": list(m["content"])} if isinstance(m.get("content"), list) else m
        for m in request["messages"]]}
    seen = 0
    changed = False
    # Hermes gives middleware an API copy. Walk newest-first across user
    # attachments and screenshots, counting image blocks, not messages.
    for message in reversed(request["messages"]):
        parts = message.get("content")
        if not isinstance(parts, list):
            continue
        for i in range(len(parts) - 1, -1, -1):
            part = parts[i]
            if not isinstance(part, dict) or part.get("type") != "image_url":
                continue
            seen += 1
            image = part.get("image_url")
            url = image.get("url") if isinstance(image, dict) else image
            if seen > IMAGES_MAX:
                parts[i] = {"type": "text", "text": "[Earlier image omitted from this call; its saved file is still available.]"}
            else:
                try:
                    resized, note = shrink(url) if isinstance(url, str) else (None, None)
                except Exception:
                    # Invalid/corrupt images and decoder limits become a
                    # visible marker, never an unbounded fail-open request.
                    resized, note = None, None
                parts[i] = ({**part, "image_url": {**(image if isinstance(image, dict) else {}), "url": resized}}
                            if resized else {"type": "text", "text": "[Image could not fit this call; its saved file is still available.]"})
                if resized and note:
                    parts.insert(i, {"type": "text", "text": note + " Apply this scaling before any earlier scale/crop mapping supplied by the tool."})
            changed = True
    return {"request": request, "source": "fragment-native-images"} if changed else None
