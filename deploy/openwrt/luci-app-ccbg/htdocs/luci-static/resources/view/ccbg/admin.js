'use strict';
'require view';

// CCBG Admin panel embedded into LuCI via an iframe.
//
// The CCBG gatewayd Admin SPA is served by the gatewayd process on the same
// host at port 61081 (see CCBG_ADMIN_BIND_ADDR). This view renders an iframe
// whose src uses the current LuCI origin's host and scheme so it works whether
// the user reaches the router via IP or hostname, on HTTP or (optionally)
// HTTPS. The gatewayd side must have CCBG_ADMIN_FRAME_ANCESTORS set to this
// same origin so the CSP frame-ancestors directive permits embedding.
return view.extend({
	load: function () {
		return Promise.resolve();
	},
	render: function () {
		var scheme = window.location.protocol === 'https:' ? 'https' : 'http';
		var host = window.location.hostname;
		var port = '61081';
		var src = scheme + '://' + host + ':' + port + '/';

		var body = E([
			E('div', { 'class': 'cbi-section' }, [
				E('h2', {}, [_('CCBG Gateway')]),
				E('p', { 'class': 'cbi-section-descr' }, [
					_('Carrier Cloud Blob Gateway admin panel. If this page is blank, ensure the gatewayd service is running and CCBG_ADMIN_FRAME_ANCESTORS includes this origin.')
				]),
				E('iframe', {
					'src': src,
					'style': 'width:100%;height:100vh;border:0;display:block;'
				})
			])
		]);

		return body;
	}
});
