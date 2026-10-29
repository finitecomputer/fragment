"""Image adapter plus a terminal classification for its own budget only."""
import sys


def register(ctx):
    if '/opt/fragment' not in sys.path:
        sys.path.insert(0, '/opt/fragment')
    import deploy_wait
    deploy_wait.install()
    ctx.register_hook('transform_api_error_classification', classify)


def classify(error=None, **kwargs):
    from deploy_wait import DeploymentUnavailable
    if isinstance(error, DeploymentUnavailable):
        # A new provider cannot fix a deployment. Keep its own deadline
        # terminal rather than handing it another retry/fallback budget.
        return {'reason': 'unknown', 'retryable': False, 'should_fallback': False,
                'should_compress': False, 'should_rotate_credential': False}
