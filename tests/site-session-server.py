"""Synthetic login/storage fixture. Credentials are fixtures, never user accounts."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import secrets
PORT = 28764
# A new session value each run: a site profile signed in on an earlier run starts signed out,
# since persistent browser profiles outlive a script's data directory.
TOKEN = secrets.token_hex(8)
BASE = f"http://127.0.0.1:{PORT}"
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        path = self.path.split('?')[0]
        authenticated = f'session={TOKEN}' in self.headers.get('Cookie', '')
        cookie = None
        if path == '/reader-feed.json':
            body = json.dumps({'version':'https://jsonfeed.org/version/1.1','title':'Reader Script Fixture','home_page_url':BASE+'/site','items':[{'id':'reader-script','title':'Reader script fixture article','url':BASE+'/reader-story','date_published':'2026-10-03T12:00:00Z','content_html':'<p>Public synthetic preview.</p>'}]}).encode()
            mime = 'application/feed+json';status=200
        elif path == '/reader-story':
            paragraph = 'Synthetic publication text about custom extraction and careful observations. '*40
            body = f'<html><head><title>Reader script fixture article</title></head><body><article><p>{paragraph}</p><p>Mozilla fixture content.</p></article></body></html>'.encode()
            mime='text/html';status=200
        elif path == '/feed.json':
            body = json.dumps({'version':'https://jsonfeed.org/version/1.1','title':'Session Fixture','home_page_url':BASE+'/site','items':[{'id':'protected','title':'Protected fixture article','url':BASE+'/protected','date_published':'2026-10-03T12:00:00Z','content_html':'<p>Public synthetic preview.</p>'}]}).encode()
            mime = 'application/feed+json';status=200
        elif path == '/paywall-feed.json':
            body = json.dumps({'version':'https://jsonfeed.org/version/1.1','title':'Paywall Fixture','home_page_url':BASE+'/site','items':[{'id':'teaser','title':'Teaser fixture article','url':BASE+'/teaser','date_published':'2026-10-04T12:00:00Z','content_html':'<p>Public synthetic preview.</p>'},{'id':'protected','title':'Protected fixture article','url':BASE+'/protected','date_published':'2026-10-03T12:00:00Z','content_html':'<p>Public synthetic preview.</p>'}]}).encode()
            mime = 'application/feed+json';status=200
        elif path == '/teaser':
            # A paywall's usual answer: 200 with the first lines and a way to sign in.
            paragraph = 'Authenticated synthetic teaser text about patient measurements and honest notes. '*40
            body = (f'<html><head><title>Teaser fixture article</title></head><body><article><p>{paragraph}</p><p>Authenticated teaser article fixture.</p></article></body></html>' if authenticated else '<html><head><title>Teaser fixture article</title></head><body><article><p>The first lines of a synthetic story.</p><p><a id="fixture-login" href="/login">Sign in to keep reading</a></p></article></body></html>').encode()
            mime='text/html';status=200
        elif path == '/protected':
            paragraph = 'Authenticated synthetic publication text about careful observations and reliable experiments. '*40
            body = f'<html><head><title>Protected fixture article</title></head><body><article><p>{paragraph}</p><p>Authenticated full article fixture.</p></article></body></html>'.encode() if authenticated else b'<html><body><p>Login required.</p><a id="fixture-login" href="/login">Login fixture</a></body></html>'
            mime='text/html';status=200 if authenticated else 403
        else:
            if path == '/login': cookie = f'session={TOKEN}; Path=/; Max-Age=3600; HttpOnly; SameSite=Lax';authenticated=True
            storage = '<script>localStorage.setItem("fixture-login","saved");indexedDB.open("fixture-login-db",1);</script>' if path=='/login' else ''
            body = f'<html><head><title>Session fixture</title></head><body><h1>Session fixture</h1><p id="fixture-session">{"Signed in" if authenticated else "Signed out"}</p><a id="fixture-login" href="/login">Login fixture</a>{storage}</body></html>'.encode()
            mime='text/html';status=200
        self.send_response(status)
        if cookie:self.send_header('Set-Cookie',cookie)
        self.send_header('Content-Type',mime);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
if __name__=='__main__':ThreadingHTTPServer(('127.0.0.1',PORT),Handler).serve_forever()
