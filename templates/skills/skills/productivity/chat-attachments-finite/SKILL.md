---
name: chat-attachments-finite
description: "Give a file to the person here with MEDIA:<absolute path> in your reply: it arrives in this chat as an attachment (a PDF, image, document, spreadsheet, audio or video) they preview and download. Use it whenever someone asks for a file, a PDF or a download, before email or a link."
---

# Files in a chat

Anything you make or find on your computer (a PDF, an image, a document,
a spreadsheet, a slide deck, audio, video, an archive) can go to the
person you are talking with as an attachment in this chat. Put its path on
a line of its own in your reply:

```
Here is the recipe card.

MEDIA:/data/work/<your profile>/chicken-fricassee.pdf
```

The line is taken out of your words and the file arrives under them, as
an attachment card: an image shows inline, audio and video play, a PDF
shows its pages, and every file has Open, Download and Share. The path is
absolute (`~/` works too). A tag inside a code block, inline code or a
quote is never sent: write it as plain text, one line per file, at most 8
files a reply and 25 MiB each.

## When

- **They are here: attach it.** When someone in the chat asks for a file,
  a PDF, a printable, an image or a download, attach it to your reply.
  Email it only when they ask for email, or it is for someone else.
- **Never answer with a path alone.** A path on your computer is nothing
  they can open: attach the file.
- **Check it first.** Look at what you made before you send it (for a
  PDF, the `pdf` skill's `scripts/pdf_page_image.py` on each page).
- **To keep it at a link**, or share it with people outside the chat,
  make it a fragment (`apps-finite`) instead, and send the link.

A file someone attaches for you is already on your computer: the note
before their message names its path.
