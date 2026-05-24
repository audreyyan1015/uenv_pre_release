class BaseAdapter:
    def convert_request(self, request):
        raise NotImplementedError

    def convert_response(self, response):
        raise NotImplementedError
