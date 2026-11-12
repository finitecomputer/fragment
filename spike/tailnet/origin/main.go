// Spike only (never merged): a test origin shaped like Fragment.
//
// One platform host P plus fragment hosts <name>.P, each its own origin.
// P serves a test page that exercises what Fragment needs over a tailnet:
// a sign-in redirect that sets cookies, subresources, a cross-origin chat
// frame (chat.P) with its own cookies, fetch (same- and cross-origin),
// WebSockets, long fetches and a large upload. Every request is logged with
// its real client address (PROXY protocol from `tailscale serve`), which is
// the leak check: everything must arrive from the client's tailnet address.
package main

import (
	"bufio"
	"crypto/rand"
	"crypto/sha1"
	"crypto/sha256"
	"crypto/tls"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"net/netip"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"
)

var (
	listen   = flag.String("listen", "127.0.0.1:8443", "TLS listen address")
	plain    = flag.String("plain", "", "optional plaintext listen address (behind a TLS-terminating proxy)")
	proxyPro = flag.Bool("proxy-protocol", false, "expect a PROXY v1/v2 header on each connection")
	certDir  = flag.String("certs", "certs", "dir of <name>.crt (full chain) + <name>.key pairs")
	platform = flag.String("platform", "", "platform host P; fragments are <name>.P")
	logDir   = flag.String("logs", "logs", "dir for access.jsonl and results.jsonl")
)

type logger struct {
	mu sync.Mutex
	f  map[string]*os.File
}

func (l *logger) write(name string, v any) {
	l.mu.Lock()
	defer l.mu.Unlock()
	f := l.f[name]
	if f == nil {
		var err error
		f, err = os.OpenFile(filepath.Join(*logDir, name), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
		if err != nil {
			log.Fatal(err)
		}
		l.f[name] = f
	}
	b, _ := json.Marshal(v)
	f.Write(append(b, '\n'))
}

var logs = &logger{f: map[string]*os.File{}}

func main() {
	flag.Parse()
	if *platform == "" {
		log.Fatal("-platform is required")
	}
	os.MkdirAll(*logDir, 0o755)
	mux := http.NewServeMux()
	mux.HandleFunc("/", route)
	h := accessLog(mux)

	if *plain != "" {
		ln, err := net.Listen("tcp", *plain)
		if err != nil {
			log.Fatal(err)
		}
		go func() { log.Fatal((&http.Server{Handler: h}).Serve(wrap(ln))) }()
	}
	certs, err := loadCerts(*certDir)
	if err != nil {
		log.Fatal(err)
	}
	ln, err := net.Listen("tcp", *listen)
	if err != nil {
		log.Fatal(err)
	}
	srv := &http.Server{Handler: h, TLSConfig: &tls.Config{Certificates: certs}}
	log.Printf("origin: P=%s tls=%s plain=%s proxy-protocol=%v certs=%d", *platform, *listen, *plain, *proxyPro, len(certs))
	log.Fatal(srv.ServeTLS(wrap(ln), "", ""))
}

func loadCerts(dir string) ([]tls.Certificate, error) {
	crts, _ := filepath.Glob(filepath.Join(dir, "*.crt"))
	var out []tls.Certificate
	for _, c := range crts {
		k := strings.TrimSuffix(c, ".crt") + ".key"
		pair, err := tls.LoadX509KeyPair(c, k)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", c, err)
		}
		log.Printf("cert %s: %v", filepath.Base(c), pair.Leaf.DNSNames)
		out = append(out, pair)
	}
	if len(out) == 0 {
		return nil, errors.New("no certificates in " + dir)
	}
	return out, nil
}

// --- PROXY protocol (v1 and v2), so the origin sees the tailnet peer ---

func wrap(ln net.Listener) net.Listener {
	if !*proxyPro {
		return ln
	}
	return &ppListener{ln}
}

type ppListener struct{ net.Listener }

type ppConn struct {
	net.Conn
	r      *bufio.Reader
	remote net.Addr
}

func (c *ppConn) Read(b []byte) (int, error) { return c.r.Read(b) }
func (c *ppConn) RemoteAddr() net.Addr       { return c.remote }

func (l *ppListener) Accept() (net.Conn, error) {
	for {
		c, err := l.Listener.Accept()
		if err != nil {
			return nil, err
		}
		c.SetReadDeadline(time.Now().Add(5 * time.Second))
		r := bufio.NewReader(c)
		remote, err := readProxyHeader(r)
		c.SetReadDeadline(time.Time{})
		if err != nil {
			log.Printf("proxy header from %s: %v", c.RemoteAddr(), err)
			c.Close()
			continue
		}
		if remote == nil {
			remote = c.RemoteAddr()
		}
		return &ppConn{Conn: c, r: r, remote: remote}, nil
	}
}

var v2sig = []byte("\r\n\r\n\x00\r\nQUIT\n")

func readProxyHeader(r *bufio.Reader) (net.Addr, error) {
	peek, err := r.Peek(12)
	if err != nil {
		return nil, err
	}
	if string(peek) == string(v2sig) {
		hdr := make([]byte, 16)
		if _, err := io.ReadFull(r, hdr); err != nil {
			return nil, err
		}
		n := int(binary.BigEndian.Uint16(hdr[14:16]))
		body := make([]byte, n)
		if _, err := io.ReadFull(r, body); err != nil {
			return nil, err
		}
		switch hdr[13] >> 4 { // address family
		case 1:
			if n >= 12 {
				ip, _ := netip.AddrFromSlice(body[0:4])
				return net.TCPAddrFromAddrPort(netip.AddrPortFrom(ip, binary.BigEndian.Uint16(body[8:10]))), nil
			}
		case 2:
			if n >= 36 {
				ip, _ := netip.AddrFromSlice(body[0:16])
				return net.TCPAddrFromAddrPort(netip.AddrPortFrom(ip.Unmap(), binary.BigEndian.Uint16(body[32:34]))), nil
			}
		}
		return nil, nil // LOCAL or unknown family: keep the socket's address
	}
	if strings.HasPrefix(string(peek), "PROXY ") {
		line, err := r.ReadString('\n')
		if err != nil {
			return nil, err
		}
		f := strings.Fields(line)
		if len(f) >= 6 {
			ap, err := netip.ParseAddrPort(net.JoinHostPort(f[2], f[4]))
			if err == nil {
				return net.TCPAddrFromAddrPort(ap), nil
			}
		}
		return nil, nil
	}
	return nil, errors.New("no PROXY header")
}

// --- access log ---

type statusWriter struct {
	http.ResponseWriter
	code  int
	bytes int64
}

func (w *statusWriter) WriteHeader(c int) { w.code = c; w.ResponseWriter.WriteHeader(c) }
func (w *statusWriter) Write(b []byte) (int, error) {
	if w.code == 0 {
		w.code = 200
	}
	n, err := w.ResponseWriter.Write(b)
	w.bytes += int64(n)
	return n, err
}
func (w *statusWriter) Flush() {
	if f, ok := w.ResponseWriter.(http.Flusher); ok {
		f.Flush()
	}
}
func (w *statusWriter) Unwrap() http.ResponseWriter { return w.ResponseWriter }

func cookieNames(r *http.Request) []string {
	var n []string
	for _, c := range r.Cookies() {
		n = append(n, c.Name)
	}
	return n
}

func accessLog(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		sw := &statusWriter{ResponseWriter: w}
		next.ServeHTTP(sw, r)
		tlsInfo := ""
		if r.TLS != nil {
			tlsInfo = fmt.Sprintf("%s/%s/%x", r.TLS.ServerName, r.TLS.NegotiatedProtocol, r.TLS.Version)
		}
		logs.write("access.jsonl", map[string]any{
			"t": start.UTC().Format(time.RFC3339Nano), "ms": time.Since(start).Milliseconds(),
			"remote": r.RemoteAddr, "host": r.Host, "method": r.Method, "path": r.URL.RequestURI(),
			"proto": r.Proto, "tls": tlsInfo, "status": sw.code, "bytes": sw.bytes, "in": r.ContentLength,
			"cookies": cookieNames(r), "origin": r.Header.Get("Origin"),
			"site": r.Header.Get("Sec-Fetch-Site"), "dest": r.Header.Get("Sec-Fetch-Dest"),
			"ua": r.Header.Get("User-Agent"), "xff": r.Header.Get("X-Forwarded-For"),
			"ts_user": r.Header.Get("Tailscale-User-Login"),
		})
	})
}

// --- routes ---

func hostOf(r *http.Request) string {
	h := r.Host
	if i := strings.LastIndexByte(h, ':'); i > 0 && !strings.Contains(h[i:], "]") {
		h = h[:i]
	}
	return strings.ToLower(h)
}

func isFragment(h string) bool { return strings.HasSuffix(h, "."+*platform) }

func route(w http.ResponseWriter, r *http.Request) {
	h := hostOf(r)
	if h != *platform && !isFragment(h) {
		http.Error(w, "unknown host "+h, http.StatusMisdirectedRequest)
		return
	}
	cors(w, r)
	if r.Method == http.MethodOptions {
		w.WriteHeader(204)
		return
	}
	p := r.URL.Path
	switch {
	case p == "/ws":
		websocket(w, r)
	case p == "/whoami" || p == "/echo" || p == "/api/cors":
		whoami(w, r)
	case p == "/slow":
		slow(w, r)
	case p == "/hold":
		hold(w, r)
	case p == "/upload":
		upload(w, r)
	case p == "/log":
		results(w, r)
	case strings.HasPrefix(p, "/static/"):
		static(w, r)
	case h == *platform && p == "/login":
		login(w, r)
	case h == *platform && p == "/":
		page(w, r)
	case isFragment(h) && p == "/frame":
		frame(w, r)
	case isFragment(h) && p == "/":
		fmt.Fprintf(w, "fragment %s\n", strings.TrimSuffix(h, "."+*platform))
	default:
		http.NotFound(w, r)
	}
}

// CORS for the platform's own fragments only (credentialed).
func cors(w http.ResponseWriter, r *http.Request) {
	o := r.Header.Get("Origin")
	if o == "" {
		return
	}
	oh := strings.TrimPrefix(o, "https://")
	if oh == *platform || strings.HasSuffix(oh, "."+*platform) {
		w.Header().Set("Access-Control-Allow-Origin", o)
		w.Header().Set("Access-Control-Allow-Credentials", "true")
		w.Header().Set("Access-Control-Allow-Headers", "content-type")
		w.Header().Set("Access-Control-Allow-Methods", "GET, POST")
		w.Header().Add("Vary", "Origin")
	}
}

func rnd() string {
	b := make([]byte, 8)
	rand.Read(b)
	return hex.EncodeToString(b)
}

func login(w http.ResponseWriter, r *http.Request) {
	next := r.URL.Query().Get("next")
	if next == "" || !strings.HasPrefix(next, "/") {
		next = "/"
	}
	// Host-only (no Domain): fragments, which run people's code, never see it.
	w.Header().Add("Set-Cookie", "sid="+rnd()+"; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=86400")
	w.Header().Add("Set-Cookie", "sid_js="+rnd()+"; Path=/; Secure; SameSite=Lax; Max-Age=86400")
	http.Redirect(w, r, next, http.StatusFound)
}

func whoami(w http.ResponseWriter, r *http.Request) {
	tlsInfo := map[string]any{}
	if r.TLS != nil {
		tlsInfo = map[string]any{"sni": r.TLS.ServerName, "alpn": r.TLS.NegotiatedProtocol, "version": fmt.Sprintf("%x", r.TLS.Version)}
	}
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	json.NewEncoder(w).Encode(map[string]any{
		"remote": r.RemoteAddr, "host": hostOf(r), "proto": r.Proto, "tls": tlsInfo,
		"cookies": cookieNames(r), "site": r.Header.Get("Sec-Fetch-Site"),
		"ts_user": r.Header.Get("Tailscale-User-Login"),
	})
}

func static(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Cache-Control", "no-store")
	switch r.URL.Path {
	case "/static/a.js":
		w.Header().Set("Content-Type", "text/javascript")
		fmt.Fprint(w, "window.__scriptLoaded = (window.__scriptLoaded||0) + 1;")
	case "/static/a.css":
		w.Header().Set("Content-Type", "text/css")
		fmt.Fprint(w, "#css-probe{width:7px;height:1px}")
	case "/static/i.svg":
		w.Header().Set("Content-Type", "image/svg+xml")
		fmt.Fprint(w, `<svg xmlns="http://www.w3.org/2000/svg" width="3" height="5"><rect width="3" height="5" fill="#3a7"/></svg>`)
	default:
		http.NotFound(w, r)
	}
}

func slow(w http.ResponseWriter, r *http.Request) {
	n, _ := strconv.Atoi(r.URL.Query().Get("s"))
	if n <= 0 || n > 600 {
		n = 5
	}
	w.Header().Set("Content-Type", "text/plain")
	w.Header().Set("Cache-Control", "no-store")
	fl, _ := w.(http.Flusher)
	for i := 1; i <= n; i++ {
		fmt.Fprintf(w, "tick %d\n", i)
		if fl != nil {
			fl.Flush()
		}
		select {
		case <-r.Context().Done():
			return
		case <-time.After(time.Second):
		}
	}
	fmt.Fprintln(w, "done")
}

func hold(w http.ResponseWriter, r *http.Request) {
	n, _ := strconv.Atoi(r.URL.Query().Get("s"))
	if n <= 0 || n > 600 {
		n = 5
	}
	select {
	case <-r.Context().Done():
		return
	case <-time.After(time.Duration(n) * time.Second):
	}
	w.Header().Set("Cache-Control", "no-store")
	fmt.Fprintf(w, "held %ds\n", n)
}

func upload(w http.ResponseWriter, r *http.Request) {
	start := time.Now()
	h := sha256.New()
	n, err := io.Copy(h, http.MaxBytesReader(w, r.Body, 512<<20))
	w.Header().Set("Content-Type", "application/json")
	res := map[string]any{"bytes": n, "sha256": hex.EncodeToString(h.Sum(nil)), "ms": time.Since(start).Milliseconds(), "remote": r.RemoteAddr}
	if err != nil {
		res["error"] = err.Error()
	}
	json.NewEncoder(w).Encode(res)
}

func results(w http.ResponseWriter, r *http.Request) {
	b, _ := io.ReadAll(http.MaxBytesReader(w, r.Body, 1<<20))
	var v any
	if json.Unmarshal(b, &v) != nil {
		v = string(b)
	}
	logs.write("results.jsonl", map[string]any{"t": time.Now().UTC().Format(time.RFC3339Nano), "remote": r.RemoteAddr, "host": hostOf(r), "report": v})
	w.WriteHeader(204)
}

// --- WebSocket (RFC 6455, enough for a spike: text echo + server ticks) ---

func websocket(w http.ResponseWriter, r *http.Request) {
	if !strings.EqualFold(r.Header.Get("Upgrade"), "websocket") {
		http.Error(w, "websocket only", 400)
		return
	}
	key := r.Header.Get("Sec-WebSocket-Key")
	sum := sha1.Sum([]byte(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))
	conn, rw, err := http.NewResponseController(w).Hijack()
	if err != nil {
		http.Error(w, "no hijack: "+err.Error(), 500)
		return
	}
	defer conn.Close()
	fmt.Fprintf(rw, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: %s\r\n\r\n", base64.StdEncoding.EncodeToString(sum[:]))
	rw.Flush()
	start := time.Now()
	var wmu sync.Mutex
	send := func(op byte, p []byte) error {
		wmu.Lock()
		defer wmu.Unlock()
		hdr := []byte{0x80 | op}
		switch {
		case len(p) < 126:
			hdr = append(hdr, byte(len(p)))
		case len(p) < 65536:
			hdr = append(hdr, 126, byte(len(p)>>8), byte(len(p)))
		default:
			hdr = append(hdr, 127)
			hdr = binary.BigEndian.AppendUint64(hdr, uint64(len(p)))
		}
		if _, err := conn.Write(append(hdr, p...)); err != nil {
			return err
		}
		return nil
	}
	done := make(chan struct{})
	ticks := 0
	go func() {
		t := time.NewTicker(time.Second)
		defer t.Stop()
		for {
			select {
			case <-done:
				return
			case <-t.C:
				ticks++
				if send(1, []byte(fmt.Sprintf(`{"tick":%d}`, ticks))) != nil {
					return
				}
			}
		}
	}()
	msgs := 0
	reason := "eof"
	for {
		var h [2]byte
		if _, err := io.ReadFull(rw, h[:]); err != nil {
			reason = err.Error()
			break
		}
		op := h[0] & 0x0f
		n := uint64(h[1] & 0x7f)
		if n == 126 {
			var b [2]byte
			io.ReadFull(rw, b[:])
			n = uint64(binary.BigEndian.Uint16(b[:]))
		} else if n == 127 {
			var b [8]byte
			io.ReadFull(rw, b[:])
			n = binary.BigEndian.Uint64(b[:])
		}
		var mask [4]byte
		if h[1]&0x80 != 0 {
			io.ReadFull(rw, mask[:])
		}
		if n > 16<<20 {
			reason = "frame too large"
			break
		}
		p := make([]byte, n)
		if _, err := io.ReadFull(rw, p); err != nil {
			reason = err.Error()
			break
		}
		for i := range p {
			p[i] ^= mask[i%4]
		}
		if op == 8 {
			send(8, nil)
			reason = "close"
			break
		}
		if op == 9 {
			send(10, p)
			continue
		}
		if op == 1 || op == 2 {
			msgs++
			send(op, p)
		}
	}
	close(done)
	logs.write("access.jsonl", map[string]any{"t": time.Now().UTC().Format(time.RFC3339Nano), "ws_closed": true, "remote": r.RemoteAddr, "host": hostOf(r), "msgs": msgs, "ticks": ticks, "secs": time.Since(start).Seconds(), "reason": reason})
}
